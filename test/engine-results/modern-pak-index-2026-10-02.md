# Modern Unreal Pak index audit — 2026-10-02

Read-only extension of the existing Unreal footer checks to **modern v10/v11
indexes**, plus legacy entry support through v9. No decompression, path
resolution, export or repacking; no game was launched or killed; no installed
asset was rewritten and no game asset is published.

## What was added

- Primary-index SHA1 was already verified for unencrypted paks. Now the
  path-hash index (PHI) and full directory index (FDI) declared in the v10/v11
  index header are read and their SHA1 values verified against the stored hashes.
- Bounded encoded-entry decoding classifies each directory entry by compression
  slot, encryption flag, stored/decoded byte counts and extension kind, without
  decompressing or resolving any path. `i32::MIN` sentinels count as deleted.
- Non-encoded entries referenced by negative directory offsets are parsed with
  the shared v1–9 entry reader and bounds-checked against the data region.
- A hash-matching but unsupported index body is reported as `UNREAL_ENTRIES|UNPARSED|…`
  rather than treated as corruption. Only a real primary-SHA1 mismatch fails.
- Legacy audits now cover v1–9, including v8/v9 positional compression names;
  empty codec slots are preserved so entry indexes map to the right name.
- Encoded offsets are bounded (`offset + stored ≤ data index start`) for
  unencrypted entries; encrypted entries are classified but not hash-checked.

New records: `UNREAL_SECONDARY|name|status|size`,
`UNREAL_MODERN_ENTRIES|entries|files|encoded_bytes|non_encoded|compressed|encrypted|deleted|stored_bytes|decoded_bytes|path_hash_seed`,
`UNREAL_METHOD|name|count`, `UNREAL_ENTRY_KIND|kind|count`.

## Real-archive survey (read-only)

Every `.pak` under the Steam library was audited. 70 files carry an Unreal
footer; 28 are encrypted (25 v11, 2 v4, 1 v7)
and were skipped at the primary hash. All **42 unencrypted v11** archives parsed
with no `UNPARSED` result. The remaining `.pak` files are Chromium/CEF or Qt Web
resources, correctly rejected as non-Unreal footers.

| Archive | Entries | Methods | Stored / decoded bytes | Secondary hashes |
| --- | ---: | --- | ---: | --- |
| Satisfactory `FactoryGame-Windows.pak` | 13,576 | 6,993 Oodle, 6,583 stored | 2,145,539,341 / 2,744,573,043 | PHI + FDI VERIFIED |
| The Dark Pictures `pakchunk0-Windows.pak` | 151,251 | classified | — | PHI + FDI VERIFIED |
| Expedition 33 `pakchunk0-Windows.pak` | 4,680 | 1,702 Oodle, 2,978 stored | 204,475,185 / 437,614,226 | PHI + FDI VERIFIED |
| Retail Hell `RetailHell-Windows.pak` | 4,479 | 1,561 Oodle, 2,918 stored | 35,473,442 / 102,123,329 | PHI + FDI VERIFIED |

## Key roadmap finding

Across all unencrypted UE5 paks, entry kinds were only config, raw image (PNG/SVG),
Wwise audio (`.bnk`/`.wem`) and other non-asset data — **no `.uasset`, `.uexp` or
`.ubulk`**. Cooked UE5 textures and materials live in the IoStore `.ucas`/`.utoc`
pair, which is still unsupported. This confirms IoStore is a first-class
prerequisite for Unreal texture work, not a later add-on. Satisfactory, for
example, ships 6,241 Wwise `.wem` entries inside its `.pak`, an audio opportunity
that also requires Wwise rather than Unreal support.

These remain integrity/classification readers. No Unreal texture or audio writer
and no Pak/IoStore repacker exists; declared Oodle codecs are not permission to
re-encode entries, and SHA1 is a corruption check, not an authenticity guarantee.

## Validation and limits

- **116 unit tests** and **7 integration tests** with real-Btrfs fixtures, plus
  **433 shell smoke checks**, all passed; Bash syntax and `git diff --check` pass.
- Synthetic fixtures cover little-endian index layouts, secondary-hash mismatch,
  directory-vs-encoded reference errors, deleted sentinels, encrypted/compressed
  classification, truncation at every index length and version bounds.
- Real validation is read-only and specific to the tested archives. Encrypted or
  signature-bearing paks stay opaque; compressed payloads are not decompressed or
  trusted. See [ROADMAP.md](../../ROADMAP.md) M2 for the IoStore/cooked-asset next
  steps; no automatic apply route was enabled.
