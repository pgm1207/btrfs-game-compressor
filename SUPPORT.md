# Tested format support and honest coverage gaps

Status: **0.3.0 released** (2026-10-08); **0.2.1** was the previous release. This
page describes the development tree. If a format or engine is not listed here as
**Stable** or **Beta**, treat it as unsupported. [ROADMAP.md](ROADMAP.md) is the
plan; this file is the evidence-based present.

## Support levels

- **Stable** — implemented, covered by automated tests, and safe to run on an
  installed library. Filesystem operations are byte-preserving.
- **Beta** — implemented for the listed subset and exercised on disposable game
  copies, but **not runtime-validated**: nobody has confirmed the game loads,
  renders, or plays correctly after the change. Opt-in only, never automatic.
- **Audit only** — read-only inspection. No writer exists; the tool will refuse.
- **Unsupported** — no reader or writer. Files are left byte-for-byte untouched.

Runtime success is never implied by an operation succeeding. Steam verification
or a fresh download is the recovery path where no restore copy is retained.

## Stable

| Capability | Scope | Evidence |
| --- | --- | --- |
| Btrfs Zstd compression | Whole-game tree; level follows the mount or an explicit 1–15 | `native/tests/filesystem.rs::real_btrfs_preserves_data_and_shares_blocks`, `test/smoke.sh` |
| Btrfs extent deduplication | Per-game, byte-verified via kernel compare | `native/tests/filesystem.rs`, `test/smoke.sh` |
| Steam library discovery | `libraryfolders.vdf`, Flatpak/native layouts, registered roots | `test/smoke.sh` |
| State, resume and `LOW YIELD` | `COMPRESSED`/`COMPACTED`/`UPDATED`/`UNCOMPRESSED`/`LOW YIELD` | `test/smoke.sh` |
| Safety guards | Never write to a running game, stop on failure, interrupt-safe | `test/smoke.sh` |
| Savings reporting | Before/after logical bytes; categories kept non-overlapping | `test/smoke.sh`, `--stats` |
| Library compaction runner | Checkpointed asset → compress → dedupe, resumable across restarts | `test/test_resume_compaction.py` |

The stable layer is engine-independent. Savings are **not** guaranteed: titles
built from already-compressed or encrypted assets may gain little or nothing.

## Beta — tested on disposable copies, not runtime-validated

| Format / engine | Supported subset | Evidence |
| --- | --- | --- |
| Loose raster images | PNG/JPEG/WebP/BMP/TGA/GIF/QOI resize under the active profile | `test/engine-results/`, `native/tests/filesystem.rs` |
| Loose DDS | In-place beta apply now uses the rich decoder/encoder: legacy BC1/2/3 and 32-bit BGRA/RGBA plus DX10 BC1/2/3/4/5/7 and R8G8B8A8/B8G8R8A8 with mip rebuild, gated on strict reduction; export-only `--texture-compress` and `--texture-compress-tree` | `native/src/texture.rs` unit tests, `native/src/assets.rs` unit tests, `test/engine-results/dds-apply-hohokum-2026-10-07.md` |
| Legacy DDS | BC1/BC2/BC3 only; simple layouts, complete mip chains required | `test/engine-results/texture-directory-readers-2026-10-02.md` |
| WAV audio | Simple mono/stereo PCM and float; profile ceilings | `test/engine-results/brotato-wav-v2.md`, `audio-main-pipeline-2026-10-02.md` |
| Godot 3 PCK | GDST `.stex` textures (supported codecs), MP3→Vorbis with import metadata | `test/engine-results/godot-balanced-steam-copies-2026-10-03.md` |
| Godot 4 PCK | GST2 textures in plain PCK v3/v4 | `test/engine-results/godot-followup-copies-2026-10-03.json` |
| Hades custom PKG | v7 same-codec lossless LZ4 | `test/engine-results/pathogenic-ultra-2026-10-02.md` |
| Standalone FMOD | FSB5 Vorbis and single-FSB RIFF/FEV banks | `test/engine-results/fmod-copy-trials-2026-10-04.json`, `fmod-installed-audit-2026-10-04.md` |

Beta writes are gated behind explicit profiles or `--assets`/`--compact-*`
commands. `apply-no-backup` is an additional explicit opt-in that retains **no**
restore copy and is irreversible except through Steam verification.

## Audit only — read-only, no writer

| Target | What is inspected | Evidence |
| --- | --- | --- |
| Unity SerializedFile v17–22 | Header, type tree, class counts, supported Texture2D fields, read-only bounds-checked resolution of declared streamed extents (same-directory only), per-format byte totals and an atlas/UI risk flag; read-only bundle node/SerializedFile inventory via `unityfs-inventory` | `test/engine-results/unity-texture-audit-2026-10-03.md`, `test/engine-results/unity-bundle-inventory-2026-10-07.md` |
| UnityFS v6–8 | Container structure; same-codec LZ4/HC is **export only** | `test/engine-results/unityfs-initial-audit.json` |
| Unreal Pak v1–11 | Footer, bounded index SHA1, entry classification | `test/engine-results/modern-pak-index-2026-10-02.md` |
| Unreal IoStore `.utoc` v1–8 | Header/counts/security/minimum extents only | `test/engine-results/iostore-headers-2026-10-02.md` |
| Godot PCK v1–4 | Structure, resource inventory, duplicate extents (dedupe export only) | `test/engine-results/godot-dedup-audit.json` |
| XNB v4–6 | Signature/header; no payload parsing | `test/engine-results/steam-library-asset-coverage-2026-10-04.md` |
| AWB/UE-style `.pak`, RE `.pak` | Inventory only | `test/engine-results/steam-library-asset-coverage-2026-10-04.md` |

Previously tested Unity stream resolution checks declared relative paths, final-file
symlinks and extent bounds using metadata only. It does **not** establish
race-resistant containment through intermediate directories, range ownership or
pixel validity. Bundle tree presence and serialized object-byte counts are also
metadata evidence, not verified texture coverage. The
[next compatibility design](docs/ENGINE_COMPATIBILITY_NEXT.md) describes the
remaining Unity and other engine work; it does not promote support levels.
The local source contains an unvalidated component-wise `openat` hardening draft;
no new containment guarantee is claimed until its deferred validation passes.

## Unsupported — byte-for-byte untouched

**Local development note:** native-only readers for XNB textures, VTF/VPK,
GameMaker FORM, Godot 4 audio, richer Unity bundle fields/references and IoStore
addressing have isolated build, fixture and selected copied-file checks. A
bounded XNB v5 Texture2D **detached lossy exporter** is also available in local
`development-audits` builds. It has no installed apply route or runtime
compatibility claim. These routes are disabled in normal builds, are not
automatic, and are not included in the tested support table.
See [draft status and limits](docs/ENGINE_DEVELOPMENT_STATUS.md).
The 2026-10-08 local hardening revision is built and its native/Python/smoke
suites pass, with real read-only Carrion XNB and Cocoon Unity audits and four
independently verified detached exports. Those are **local development results**,
not runtime certification: prior build and copied-file evidence still does not
establish installed playability or physical savings. See the
[review](docs/ENGINE_HARDENING_REVIEW_2026_10_08.md).

These are the honest gaps. The tool detects and skips them; it does not guess.

- **Unity**: packed Texture2D/`.resS` rewriting, SpriteAtlas/UI protection,
  bundle/catalog integrity rebuilding, embedded AudioClip rewrite. Signed/custom
  integrity checks remain blockers, not something the tool can generically re-sign.
- **Unreal**: **all** cooked `.uasset`/`.uexp`/`.ubulk` texture and SoundWave
  rewriting, Pak/IoStore repacking, chunk/block decoding, virtual textures,
  cubes/arrays, normals/HDR.
- **Godot**: Godot 4 audio and non-GST2 resources, embedded packs, encrypted or
  sparse layouts, Basis/ETC/ASTC/half-float, PCK v2 apply (v1–4 are read-only).
- **Middleware**: Wwise WEM/BNK/PCK, CRI ADX/HCA/AWB/ACB, loose Ogg/MP3/FLAC/
  Opus/AAC re-encode, Bink 1/2 and other game video.
- **Other engines**: Source 1/2, GameMaker, RPG Maker, Ren'Py, XNA/MonoGame,
  Creation/Gamebryo, id Tech, Frostbite, RE Engine, RAGE, REDengine, Decima and
  the remaining families in [ROADMAP.md](ROADMAP.md) have **no dedicated packed
  adapter**. Only the generic loose-file subset above applies.
- **Codecs depending on proprietary tools** (for example Oodle) stay skipped by
  design; no external encoder is bundled.
- **Anti-cheat**: never detected or accommodated. Do not use beta asset writes on
  online/anti-cheat titles.

## Why "all assets, all engines" is not a promise

Every writer must pass the acceptance gate in [ROADMAP.md](ROADMAP.md): malformed
fixtures, independent re-parse and unchanged-resource verification, repeated-apply
stability, physical measurement, a manual game-copy playtest, and documented
bounded scope. Encrypted or signed packs, custom formats and proprietary codecs
will remain explicit skips rather than silent guesses.

The realistic target is a stable program with **explicitly tested coverage and
honest gaps**, expanded engine by engine. That is what this page tracks.
