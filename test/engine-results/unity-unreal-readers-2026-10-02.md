# Unity/Unreal reader foundations — 2026-10-02

These are **read-only audits**, not texture/audio compression implementations.
No game was launched or killed. No installed game was rewritten, and no game
assets are included in the repository. Root access was not needed.

## Unity SerializedFile metadata

`--audit-container FILE` now routes plausible standalone v17–22 SerializedFiles
to the bounded native metadata reader. Both endian layouts are handled. It checks
file length, metadata/data bounds, type indexes, unique object IDs and nonoverlap
of object extents. Metadata is capped at 32 MiB; counts are bounded. Unknown future
versions, truncation, invalid text/booleans and unknown nonzero trailers fail closed.

It inventories engine/platform/version/type-tree evidence and class counts. It
skips type-tree blob contents structurally and leaves all object payloads opaque;
it is not yet a semantic type-tree or Texture2D parser. External-reference counts
do not mean `.resS`/`.resource` references have been resolved.

Read-only trial: Mech Havoc `MechHavoc_Data/sharedassets0.assets`.

| Metadata | Value |
| --- | --- |
| SerializedFile format | 22 |
| Engine build string | 6000.0.59f2 |
| Platform ID | 19 |
| Metadata endian | little |
| Metadata bytes | 3,007,403 |
| File bytes | 251,098,204 |
| Data offset | 3,007,456 |
| Type trees | absent |
| Serialized types | 183 |
| Objects | 124,945 |
| External references | 3 |
| Texture2D objects | 506 |
| AudioClip objects | 145 |
| Sprite objects | 1,453 |
| SpriteAtlas objects | 1 |

Texture2D object records total 2,177,812 bytes, but **this is not total texture
storage**: external streams are not attributed yet. The absent type trees and
sprite/atlas objects demonstrate why a dimension-only rewrite is inappropriate.
The next stage must resolve exact versioned schemas and streaming/atlas references.

Output: `UNITY_SERIALIZED|format|file_bytes|metadata_bytes|data_offset|endian|engine|
platform|has_type_trees|types|objects|external_count`, followed by
`UNITY_CLASS|class_id|kind|objects|object_record_bytes` records. The displayed
schema wraps here for readability; native output has one line per record.

## Unreal Pak primary-index checks

Footer v1–11 inspection now streams the format's SHA1 check over unencrypted
primary-index bytes up to 256 MiB. A mismatch is an error. Encrypted or larger
indexes are explicitly skipped. `.sig` companion presence and frozen-index flags
are reported. SHA1 matches the archive format; it is not a signature/authenticity
guarantee. Secondary index hashes and entry payloads are not parsed or verified.

| Read-only trial | Pak version | Primary index bytes | Result | `.sig` present |
| --- | ---: | ---: | --- | --- |
| Satisfactory `FactoryGame-Windows.pak` | 11 | 345,282 | VERIFIED_PRIMARY_SHA1 | yes |
| Trover `Trover-WindowsNoEditor.pak` | 7 | 7,567,280 | SKIPPED_ENCRYPTED | yes |

Satisfactory declares Oodle in the footer; this does not prove per-entry usage
or provide a compatible encoder. Both signature companions are future writer
blockers, not successfully verified signatures. Trover's encrypted index remains
opaque. Neither trial permits safe texture compression or repacking yet.

Additional audit records: `UNREAL_INDEX|status|primary_index_bytes` and
`UNREAL_SECURITY|encrypted_index|signature_companion_present|frozen_index`.

## Validation and limits

- **101 Rust unit tests passed**, including v17–22/endian/type-tree combinations,
  every metadata truncation of a fixture, bad headers/object IDs/type indexes/
  extents/text, primary-index corruption/truncation and encryption/budget skips.
- **7 integration tests passed** with Btrfs fixtures enabled, including the new
  CLI/content-detection/read-only/symlink/known-SHA1/corruption test. The existing
  FMOD unit apply/restore test also executed on real Btrfs (not early-return skipped).
- **433 shell smoke checks passed**, plus Bash syntax and `git diff --check`.
- Static production backend rebuilt using `make native`.
- These trials validate metadata/integrity reader routes only. They make no
  physical-savings, pixel-quality, audio-quality or runtime-compatibility claim.
- See [ROADMAP.md](../../ROADMAP.md) for staged schema/codec/reference/export/apply
  work; no Unity/Unreal production writer has been enabled.
