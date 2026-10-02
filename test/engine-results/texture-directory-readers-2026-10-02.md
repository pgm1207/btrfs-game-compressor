# Texture fields and legacy Pak directories — 2026-10-02

This increment adds **readers only**, not Unity/Unreal texture compression.
No games were launched/killed, no installed assets were rewritten, no credentials
or game assets were published, and no elevated access was needed.

## Unity type-tree Texture2D fields

The SerializedFile reader retains bounded Texture2D type-tree records and object
extents. A separate walker handles supported primitive/struct/string/byte-vector
layouts with alignment, hierarchy, string and work limits. It extracts dimensions,
numeric format ID, mip count, inline data length and declared stream path/range.

- Root/numeric schema checks, full-object consumption and duplicate field checks.
- 4,096 nodes and 128 KiB decoded tree text per tree; at most 128 retained trees
  and 32,768 total retained nodes, shared across objects rather than cloned.
- Up to 10,000 texture objects; at most 8 MiB per object and 64 MiB payload reads
  per file. Arrays/work have separate limits; byte vectors are skipped directly.
- Unknown common strings/leaf types, malformed counts/hierarchy, invalid mip
  dimensions, inconsistent storage and absent trees remain opaque.
- Paths are **not followed**, payloads are **not decoded**, and external stream
  existence/bounds/reference ownership are **not verified**.

New records:

`UNITY_TEXTURE|path_id|width|height|format_id|mips|inline_bytes|stream_offset|stream_size|declared_path`

`UNITY_TEXTURE_AUDIT|inspected_objects|opaque_or_budget_skipped_objects`

Synthetic fixtures exercise little/big endian, raw and byte-vector image fields,
every payload truncation, duplicate/schema fields, dimensions, overflow, future
types and complete flow from a SerializedFile metadata table through audit. The
synthetic external companion intentionally does not exist; audit succeeds without
attempting to resolve it. This is not a vendor-built runtime compatibility fixture.

Mech Havoc's original Unity 6 `sharedassets0.assets` was re-audited: its 506
Texture2D objects still report **0 inspected / 506 opaque**, because its type trees
are absent. There is no version-name heuristic, trial-and-error schema or guessed
writer. Stripped schemas and stream/reference resolution are the next milestones.

## Unreal legacy index/header/stored-payload audit

After primary-index SHA1 succeeds, unencrypted Pak v1–7 indexes up to 32 MiB can
be inspected. The reader validates bounded UTF-8/UTF-16 strings, unique names,
entry data bounds/nonoverlap, known flags and compression-block ranges, then
compares each nondeleted data header against its indexed record. It reports actual
indexed method IDs (not inferred footer usage) and `.uasset`/`.uexp`/`.ubulk` counts.

Stored, unencrypted payload SHA1 is checked within a 64 MiB total read budget.
Compressed/encrypted payloads are explicitly skipped; there is no decompressor,
extractor, repacker or cooked-asset writer. Deleted entries are counted separately.
Modern/frozen index layouts are not reinterpreted as legacy directories.

New records:

`UNREAL_LEGACY_ENTRIES|entries|stored|encrypted|deleted|headers_checked|stored_hashes_checked|payloads_skipped`

`UNREAL_METHOD_ID|method_id|entries` and `UNREAL_ENTRY_KIND|kind|entries`.
Unsupported version/encryption/budget uses an explicit skip-status record.

Synthetic valid v1–7 fixtures test stored data/header hashes, every index truncation,
header and payload corruption, future versions, budget/flag failures and UTF-16
lengths. Separate fixtures prove encrypted entries are not decrypted/hashed and
compressed blocks are range-checked without declaring their payloads verified.
The existing CLI SHA1-only fixture now uses a modern v11 footer: arbitrary
`abc` index bytes are suitable for an isolated SHA1 check, **not a valid legacy
directory**. It must not masquerade as a successful directory test.

Read-only real-file checks:

| Archive | Result |
| --- | --- |
| FighterZ `pakchunk1-WindowsNoEditor.pak`, v4 | Encrypted index, `.sig` present; primary/directory checks skipped |
| Satisfactory `FactoryGame-Windows.pak`, v11 | Primary SHA1 verified, `.sig` present; legacy directory check skipped |

No unencrypted commercial v1–7 directory was independently validated in this
increment. Synthetic passes do not certify all legacy archives or gameplay.
Signature presence is a blocker for future writers, not verified authenticity.

## Validation

- **111 unit tests**, **7 integration tests** with real-Btrfs fixtures enabled,
  **433 shell smoke checks**: all passed.
- Static backend and release package rebuilt; `git diff --check` passed.
- No new dependencies; no automatic Unity/Unreal apply route enabled.
- Next: validated stripped schemas, stream/atlas reference ownership, modern Pak
  directory/secondary hashes, then codec-preserving texture exports with independent
  verification. Main-pipeline writes remain behind the roadmap acceptance gates.
