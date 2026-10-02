# Slay texture protection trial — 2026-10-02

The user reported that Ultra Performance looked acceptable for characters and
backgrounds but made card art too blurry. Inspection of the original pack found
three card atlases at 4032×4072, 4032×4032 and 3528×3080. One `AtlasTexture`
resource uses a 250×351 region: applying a 640-pixel cap to the whole first sheet
leaves that region approximately 39×55. A minimum *file* size or a small-sheet
cutoff alone cannot protect these individual sprites.

## Rules implemented in the backend

- Every lossy profile preserves textures whose longest edge is ≤512 pixels or
  shortest edge is ≤64 pixels, including their original colour precision.
- Skip resizing if the output's shortest edge would be <64 pixels; never upscale.
- Preserve known atlas names, and packed Godot textures referenced by plain
  `AtlasTexture` resource metadata, resolving source paths through `.import`
  stubs. All aliases of a protected payload are protected.
- The shared guards cover loose raster/DDS and packed Godot 3/4 textures. Export
  and main asset apply paths use the same guards. Native/Lossless stay unchanged.
- Unknown/unnamed atlases or compressed metadata may evade detection. No claim
  of universal semantic UI/card detection is made. Metadata scans are bounded;
  exceeding the budget rejects the packed texture pass rather than silently
  ignoring potentially relevant declarations.

## Copy-only Ultra Performance trial

Original installed pack: 1,901,047,880 bytes,
SHA256 `faea5eabbbebf4d9ca9997b25af9c8c8517c07c27bcd41588187d2dd8d40b296`.

Created a new reflink copy and ran the **main asset apply pipeline**, not a
manually selected texture export:

```sh
./bgc-native assets apply ultra-performance 3 /mnt/storage/Games/bgc-slay-protected-ultra-20261002
```

Candidate: 945,360,232 bytes (50.27% logical reduction), 1,300 changed textures.
Old unguarded Ultra Performance candidate: 874,440,584 bytes. This protection
costs 70,919,648 logical bytes while keeping the card sheets at native quality.

Candidate SHA256:
`e04046df9de3cf2105a33abcb232a0a60ff5a0cb3e71e4daea183d4a721abeb4`.

- Independent `verify-godot4-textures.py SOURCE CANDIDATE 640`: PASS (all hashes,
  unchanged resources, logical dimensions, codecs and mip chains checked).
- Separate original/candidate byte comparisons: all three card atlases, 2,026
  small textures and 235 thin textures unchanged (small/thin sets overlap).
- Second apply: `NO_GAIN`, no textures changed.
- Installed pack remains unchanged; no games launched or killed.
- 90 unit tests and 433 smoke checks pass. Optional real-Btrfs integration tests
  were not enabled for this run.

Manual visual comparison is still pending. Steam launch option for this trial:

```text
--main-pack /mnt/storage/Games/bgc-slay-protected-ultra-20261002/SlayTheSpire2.pck
```

Remove the option to use the original installed pack. Existing Ultra Performance
and Balanced candidates were not overwritten or deleted.
