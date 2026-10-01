# In-place packed PCK validation (2026-10-02)

This validates the new **in-place** packed-asset path (`assets apply` rewrites a
supported standalone Godot PCK directly, with no per-file backup) against copies
of real installed packs. The installed games were never modified.

## Method

Scratch tree: `/home/pgm1207/bgc-realpack-test` (Btrfs, same filesystem as the
games). Each candidate was `cp --reflink=auto` from the installed pack, then:

```bash
bgc-native assets apply ultra-performance 3 <scratch-dir>
```

Release binary, Zstd level 3. Verify with the existing independent checker:

```bash
python3 test/verify-godot4-textures.py <original> <candidate> 640
```

`bgc-native assets apply` was run twice per pack to test idempotency. Recovery is
Steam "Verify integrity of game files"; no `.bgc-assets-backup` is created for
packed assets.

## Results

| Game | Engine / pack | Original bytes | Rewritten bytes | Textures | Verifier | Second pass |
| --- | --- | ---: | ---: | ---: | --- | --- |
| Brotato | 3 / v1 | 64,625,178 | 64,625,178 | 0 | n/a (already optimized) | SHA unchanged |
| Pathogenic | 4.7.0 / v4 | 1,440,681,380 | 555,096,852 | 1,704 | PASS | byte-identical no-op |
| Slay the Spire 2 | 4.5.1 / v3 | 1,901,047,880 | 874,440,584 | 1,997 | PASS | byte-identical no-op |

Candidate SHA256:

- Brotato: `54b634fa09f573a4b12b5ba64e75b215269ca1f8bdd648c5a5a05c9b7bbfe0cb`
  (unchanged; matches the published installed pack).
- Pathogenic: `445c0773448474d7b771f0b1082cfe393fff13492aa481ddc9cb5ae15ce2cd9b`.
- Slay the Spire 2: `7eee8da10631e60b42f24db28a4b4ec9afd6f8e75cf791def34283e7138f1fc1`.

The Slay in-place candidate is **byte-identical** to the export candidate
recorded in `godot-library-2026-10-01.md`, so the in-place path reproduces the
previously playtest-validated bytes.

Physical `du` tracked logical almost exactly (e.g. Pathogenic 555,096,852 ->
555,098,112 disk bytes): the GST2 payloads are already entropy-coded, so the
Btrfs Zstd recompression step gains almost nothing on a packed PCK. The win is
the logical rewrite itself, not compression.

## Idempotency bug found and fixed

The first real-pack run changed the pack again on the second `assets apply`
(Pathogenic 555,070,356 -> 554,881,428, ~0.03%), then converged. Analysis of the
intermediate showed the base mip level was byte-identical and **only the
regenerated mip levels differed**: mips were derived from the pristine decoded
base on pass 1 and from the re-decoded, already-quantized base on pass 2.

Fix in `native/src/gst2.rs::transform`:

- Derive each mip from the already-quantized level above it, not the pristine
  base, so the transform is a fixed point.
- Skip the resize when it is a 1:1 no-op (Lanczos3 is not an identity op).

After the fix, Pathogenic and Slay both report `NO_GAIN` on a second pass with a
byte-identical SHA, and both verifiers still pass. Regression test:
`gst2::tests::at_cap_textures_are_quantized_once_and_then_untouched`.

The Pathogenic export number changed by 26,496 bytes (555,070,356 ->
555,096,852) because its mipmapped textures now build mips from the quantized
parent; Slay was unaffected (byte-identical). Brotato uses the separate GDST v1
path and is unchanged.

## Caveats

- In-place writes keep no per-file backup. A bad rewrite relies on Steam verify,
  which may re-download a large pack.
- "480p" is the chosen label; the implementation is a 640-pixel longest-edge
  texture cap, not a render-resolution change.
- Slay was validated on a copy only. The installed pack was left untouched;
  manual playtest remains the user's step.
