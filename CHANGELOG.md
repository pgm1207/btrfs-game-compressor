# Changelog

## 0.3.1 — 2026-10-08 — Recovery hardening, measurement and DDS quality

This maintenance release contains the changes listed below. The release artifacts
are available to installers **only after** the `v0.3.1` tag triggers release CI
and the release uploads complete; a merge to `main` alone does not ship them.

### Safety and compatibility

- Normal Godot PCK beta apply now stages each optimized archive separately,
  retains a local original with a durable recovery checksum, and supports
  exact-byte restore/finalize (#34). Explicit no-backup compaction remains
  irreversible; whole-game crash/power-loss atomicity is not yet guaranteed.
- Harden rollback: retain prune originals if parent-directory sync fails after
  unlink; never remove unrelated checksum/staging files on a create-new filename
  collision (#37).
- Serialize cooperating native asset mutations per game with automatic-release
  advisory inode locks (apply, restore, finalize, per-file restore, prune).
  This does not lock Steam or prevent concurrent Steam updates (#38).
- Constrain Godot packed-asset mutation to the selected game root via
  descriptor-anchored `openat2` path checks; reject cross-device backup
  directories and unexpected nested mounts (#39).
- Surface packed-asset operational I/O failures (ENOSPC, permissions, staging
  collisions) instead of silently treating them as unsupported pack skips
  (#40). Unsupported/invalid formats remain beta skips.
- Native-only asset apply/restore can now run on non-Btrfs Linux filesystems
  such as ext4, but main Steam-library discovery and filesystem compression/
  deduplication still require Btrfs. ext4 backups can cost a full copy (#36).
- Preserve **all platform-specific asset directories** during `--slim` until
  the game runtime can be independently established. SteamOS can launch native
  Linux titles or Windows titles through Proton, so filesystem/host OS alone is
  not evidence for deletion (#1).
- Correct default Steam Deck storage claims. Standard Steam Deck installations
  typically use ext4; only explicitly Btrfs-formatted game libraries are eligible
  for Btrfs compression/deduplication (#2).
- Give the actual Steam update policy: `Only update this game when I launch it`
  defers background changes and cannot permanently disable updates. Show the
  advisory after supported asset apply/slim and before irreversible library-wide
  compaction; remind users to revalidate lossy assets after Steam updates (#2, #21).
- Avoid reading an invisible **second confirmation** when the TUI already asked
  the user to approve asset optimization (#4).
- Improve running-game heuristics by checking accessible `/proc/PID/cwd` in
  addition to memory mapped file paths; document that process checks are
  best-effort and cannot atomically prevent later launches (#22).

### Compression, quality and usability

- Add read-only `bgc-native texture-quality ORIGINAL.dds CANDIDATE.dds`
  codec-distortion metrics, with alpha-aware black/white PSNR and altered-alpha
  pixel count. These do not measure lost source resolution, game quality or
  runtime compatibility (#24).
- Add a separate native-resolution DDS quality comparison: upsample the
  candidate with Lanczos3 and compare against the original to expose spatial
  downscaling losses distinct from codec-only PSNR. Pixel scores still do not
  certify perceptual/gameplay quality (#42).
- In installed beta DDS apply, skip same-size lossy recompression and reject
  too-thin downsized textures before expensive decoding. Detached DDS export is
  still explicit and separate. The half-original-dimension budget is not yet
  implemented for this loose DDS route (#5).
- Reuse **byte-identical encoded lower DDS mip levels** when a requested size
  exactly matches an existing mip chain level. This avoids an additional codec
  generation but **does not preserve lost spatial resolution** or establish
  game compatibility (#19).
- Add `--asset-plan-dir DIR`, a read-only Balanced asset-candidate scan for
  arbitrary real directories, including ext4 Steam libraries. It does not
  perform filesystem compression or enable asset writes outside Btrfs (#6).
- Add native end-to-end read-only planner tests using a regular temporary
  filesystem and symlinked root rejection (#20).

### Observed storage reporting

- Add `tools/space-delta.py` with versioned before/after JSON observations:
  report filesystem-wide available-space deltas separately from Btrfs extent
  reductions, preserve negative changes, and use `null` for unavailable
  measurements instead of inventing zero savings (#35).
- Add an unprivileged read-only `bgc-native fs-space PATH` statvfs report and
  an observed available-space delta around interactive asset apply/restore/
  finalize. The result is filesystem-wide, not an attributable game savings
  measurement; concurrent writes, snapshots and backup storage may influence
  the observed change (#23).

### Testing

- Add CI that creates a disposable loopback Btrfs mount and actually runs the
  opt-in native filesystem integration tests, instead of silently passing with
  those cases skipped (#3).

**Still unresolved:** whole-game power-loss-safe transactions, runtime
compatibility certification of lossy Godot/Unity/Unreal modifications, absolute
protection against concurrent Steam changes, causal net reclaimed-space
attribution, full ext4 Steam-library/TUI support, GUI/controller workflows and
signed releases. Track these in GitHub issues #7–#18 and #31.

## 0.3.0 — 2026-10-08 — Texture compression MVP and Unity bundle inventory

- Add a gated, default-off `development-audits` build with native-only draft
  readers for uncompressed XNB v5 Texture2D, PC VTF 7.1–7.4, Valve VPK v1/v2,
  GameMaker FORM, richer Unity bundle fields, Godot 4.3 audio and IoStore v3–8
  addressing, plus a **detached** XNB v5 BC1/2/3 Texture2D resizing exporter.
  Normal builds refuse these routes before opening an input; no installed apply,
  automatic routing or Bash UI is enabled.
- Detached export safety: immutable in-memory reparsing (shared parser), a
  byte-identical no-change rebuild check, BC1 one-bit alpha retention, and
  anonymous `O_TMPFILE` staging published with a no-replace `linkat` so a partial
  or replaced destination is never visible. Sources are never modified.
- `tools/asset-opportunity-report.py` schema 3 keeps experimental XNB estimates
  out of production totals and records audited/header coverage and partial errors
  separately, so research failures do not erase production results. Schema-2
  reports are preserved, not migrated.
- Unity read-only audit validates bounded type-tree hierarchy and local strings
  before unknown common-string semantics (separating malformed/unsupported/budget
  statuses), builds subtree ends in linear time, and no longer allocates field
  paths for skipped array elements. A reference/ownership design contract records
  the unresolved Sprite/SpriteAtlas/PPtr and stream-ownership gates.
- Add an export-only DDS texture compressor: `--texture-compress MAX_DIM FILE
  OUTPUT` (native `texture-compress`) downscales the base mip of a bounded 2D DDS
  to a maximum dimension, re-encodes it to the same codec with a regenerated mip
  chain, and verifies the written file by re-parsing and re-decoding it. Supports
  legacy BC1/BC2/BC3 and 32-bit BGRA/RGBA plus DX10 BC1/BC2/BC3/BC4/BC5/BC7 and
  R8G8B8A8/B8G8R8A8. Cubemaps, arrays, volumes, BC6H and float formats are
  refused. Never writes in place; the source is untouched and the output is a new
  file. The payload is lossy and unverified in-game, and this is not main-pipeline
  apply or net savings. `--texture-compress-tree MAX_DIM DIR OUTPUT_DIR` (native
  `texture-compress-tree`) mirrors a whole tree, exporting only textures that
  actually shrink and reporting skipped/failed counts; the source tree is never
  modified.
- Route `.dds` in the asset planner/apply path through the same richer decoder
  and encoder, so multi-mip, DX10 (BC1/2/3/4/5/7) and uncompressed 32-bit
  BGRA/RGBA textures are now in-place candidates under the existing profile,
  atlas/small/thin guards and restore or no-backup modes. Replacement is still
  gated on a strict byte reduction. Validated on disposable copies: a 1920x1080
  BC3 and a 1920x1080 BGRA DDS each shrank ~85% in place, while a small texture
  and a non-texture file were left untouched.
- Report a read-only Godot 3 `.stex` histogram (`PCK_STEXTURE`: data-format word,
  mip count, count, bytes, max dimensions) from `--audit-container`, so the
  encodings behind the Godot 3 texture gap are visible. Real Godot 3 `.stex` in
  this library are mostly small single-mip WebP sprites the policy correctly
  refuses to downscale; multi-mip `.stex` remain skipped. The large supported
  wins are Godot 4 `.ctex` (see `test/engine-results/godot-texture-opportunity-2026-10-07.md`).
- Add a read-only `unityfs-inventory` command: decode a UnityFS bundle in memory,
  classify each node (SerializedFile, `.resS` stream, `.resource`, opaque) and
  summarize the SerializedFiles. Real Addressables bundles report **type trees
  present** with readable Texture2D objects, unlike stripped standalone player
  builds, so Texture2D metadata is available where the texture bytes live. No
  payload is rewritten and no writer exists yet
  (`test/engine-results/unity-bundle-inventory-2026-10-07.md`).
- Unity read-only audit now resolves declared Texture2D stream paths against the
  audited file's own directory and bounds-checks `offset .. offset+size` without
  reading payload bytes. Absolute paths, `..` traversal, symlinks and
  non-regular files are refused, and shared streams are grouped per stream file.
  This is groundwork for a streamed-texture writer and enables no writer by
  itself.
- The same audit now aggregates declared inline and streamed bytes per
  TextureFormat and emits a file-level `UNITY_ATLAS_RISK` flag when SpriteAtlas
  or Sprite objects are present, so a future writer must protect atlas/UI
  textures. Totals cover only textures with a supported schema.

## 0.2.1 — 2026-10-05 — Correct open flags on aarch64; release fix

- Use `libc::O_NOFOLLOW`, `libc::O_NONBLOCK` and `libc::O_DIRECTORY` instead of
  hardcoded magic numbers on every source-open and destination-create path. The
  value `0x20000` is `O_NOFOLLOW` on x86_64 but a different flag on aarch64, so
  the 0.2.0 release build followed symlinks there and failed its own
  read-only-audit tests. This corrects the flag on every architecture.
- Keep an explicit `symlink_metadata` gate on the UnityFS and FMOD source readers
  as a flag-independent second check.
- 0.2.0 failed its aarch64 release build and published no artifact; 0.2.1 is the
  first 0.2.x release.

## 0.2.0 — 2026-10-05 — Native backend and compatibility-first assets

First stable 0.2.0 release. The filesystem layer (Zstd compression, byte-verified
deduplication, discovery, state and safety guards) is stable and tested. Engine
asset support is **beta** and unverified at runtime; [SUPPORT.md](SUPPORT.md)
records exactly which formats are tested, audit-only or unsupported, and every
writer remains opt-in.

### Release highlights

- Publish [SUPPORT.md](SUPPORT.md): an evidence-based support matrix with stable,
  beta, audit-only and unsupported tiers, plus the explicit list of coverage gaps
  (Unity/Unreal packed writers, Wwise/CRI, video, encrypted packs).
- Add `--compact-all-no-backup`: a checkpointed installed-library Balanced asset
  → Zstd → dedupe pipeline that resumes across restarts, never re-applies an
  uncertain asset stage, and writes a savings chart.
- Add explicit `apply-no-backup` native asset mode using synced temporary files
  and atomic replacement, without persistent recovery copies. Existing backups
  remain untouched; Steam verification is the recovery path. Cover no-backup
  resizing on opt-in real Btrfs fixtures.

### Development improvements

- Validate library-wide inventory/planner rows and return failure on incomplete
  scans. Count only actual logical reductions as candidates, not recognized
  no-gain packs; explicitly distinguish estimates from runtime approval.

- Fix PCK v3/v4 audit-report directory pointers: read the offset at byte 32,
  not the flags/file-base fields at byte 20. Bound count reads against file size
  and test all four versions, truncated pointers, and overflow.

- Expand `--stats` into a measured savings-by-category breakdown, sorted by
  reduction percentage. Show bytes and each category's own baseline, keep dedupe
  scope separate to avoid double counting, and explicitly mark language/asset
  history as untracked until durable accounting is added.
- Run the complete Balanced (1080p) asset pipeline on seven disposable Steam
  copies spanning Godot PCK v1/v2/v3/v4. Six were accepted and audited; rewrites
  were idempotent on repeat. No installed game was modified.
- Add read-only installed-game asset inventories, standalone-container inventory
  and Godot PCK structure-audit reports; add copy-only PCK trial coverage. These
  distinguish detected containers from rewrite eligibility and preserve the
  explicit warning that structural audits do not establish game loading.
- Bound and scale Godot PCK audits for large installs (including 211k-entry
  Until Then), report correct version-specific directory counts, and reject
  extreme declared counts before allocation. Preserve duplicate exact extents
  because PCK deduplication legitimately emits shared ranges.
- Add a read-only XNB v4–6 header audit and signature-validated XNB container
  inventory. Do not infer XNA/MonoGame/FNA from XNB files or parse their payloads.
- Route recognized FSB5 and FMOD RIFF/FEV banks through the existing bounded
  FMOD audit in `container-audit`, rather than misclassifying them as Unreal Pak.

- Name recognized Unity TextureFormat IDs in the read-only Texture2D inventory,
  while retaining the numeric value and reporting unrecognized IDs as `Unknown`.
  This is enum metadata only; it does not validate payload bytes or enable writing.

- Use bounded streaming atlas metadata inspection for large Godot scenes;
  preserve cross-chunk resource paths and fail closed on scan/path budgets.
  A real Pathogenic trial now exports where the former 4 MiB limit refused.
- Stabilize BC texture repeats: account for original block padding in logical
  size budgets and skip quantization-only lossy BC re-encoding at the cap.
  Pathogenic Ultra Performance reduces its pack by 45.4%, with a zero-change
  repeat. Physical measurements and user playtesting remain pending.

- Add content-detected, read-only IoStore `.utoc` header inventory: TOC versions
  1–8, security/count/partition metadata and version-aware minimum extents.
  Surveyed 69 real v6/v8 headers; companions are metadata-only and links are
  never followed. No chunk decoding, signature verification or writer enabled.

- Audit modern Unreal Pak v10/v11 indexes read-only: verify path-hash and
  directory-index SHA1, then classify bounded encoded entries (compression slot,
  encryption, stored/decoded bytes, entry kinds) without decompressing. A
  hash-matching unsupported body is reported as UNPARSED, not corruption.
- Extend unencrypted legacy audits from v1–7 to v1–9 with positional compression
  names for v8/v9 entries and explicit empty-slot handling.
- Confirm across 42 real unencrypted UE5 paks that shipped `.pak` files hold
  config/Wwise/raw media rather than cooked `.uasset` textures; those live in
  IoStore `.ucas`/`.utoc`, whose cooked payloads remain unsupported.

- Inspect bounded type-tree Texture2D metadata: dimensions, format ID, mips,
  inline bytes and declared stream ranges. No stripped-schema guessing, stream
  path traversal, texture decoding or writing.
- Extend unencrypted legacy Pak v1–7 audits with directory/data-header checks,
  indexed method/entry-kind counts and bounded stored-payload SHA1 verification.
  Unknown flags/overlaps/corruption fail closed; compressed/encrypted payloads,
  modern indexes and cooked asset writers remain unsupported.

- Add bounded standalone Unity SerializedFile v17–22 metadata/class audit through
  `--audit-container`, including stripped-player version/type-tree evidence.
  Texture/audio payloads remain opaque; no Unity writer is implied.
- Verify bounded unencrypted Unreal Pak primary-index SHA1; explicitly report
  encrypted/budget skips, signature-companion presence and frozen indexes.
  Entry/secondary-index/signature verification and cooked texture/IoStore writers
  remain unimplemented.
- Publish the engine/format support chart, staged Unity/Unreal roadmap and
  incremental version/release policy. Fetch locked dependencies on clean release
  runners before offline builds.

- Apply supported standalone Godot 3/4 PCK transforms through the main pipeline.
  Chain Godot 3 audio and textures, preserve unknown resources, verify finished
  packs, and make repeated texture applies stable. Packed PCK recovery uses Steam
  verification; loose sources retain restorable backups.
- Make small/thin texture protection, known-atlas protection, relative half-size
  budgets and a 7-bit RGB rounding floor automatic across supported lossy paths.
  Profile texture caps are soft targets, not game render resolutions. Godot 4
  textures without reliable original/logical dimensions are skipped.
- Share audio quality policy across WAV, Godot 3 audio and supported standalone
  FMOD FSB5 Vorbis banks. Automatic FMOD applies are bounded, savings- and
  waveform-gated, preserve playback metadata and use loose-file backups. Native
  and Lossless never transcode audio; embedded Unity/Unreal audio stays untouched.
- Verify Mech Havoc lossless copy bytes and exact Btrfs extent measurements:
  90.23% less physical data than logical bytes, or 19.35% improvement over its
  already-compressed installation. Do not conflate this with net free-space gain.
- Validate conservative Slay texture copies, Hades bank apply/repeat/restore,
  96 unit tests, 6 enabled real-Btrfs integration tests and 433 shell smoke checks.
  Runtime visual/audio quality still requires manual playtesting.

### Earlier development history

The export-only notes below describe the initial implementation stages, not the
current automatic-apply support. See README for current supported formats.

- Initial Godot support: plain PCK v1–4 inventory/deduplication, Godot 3
  GDST and Godot 4 GST2 profile-aware texture exports, plus Godot 3 music/PCM
  exports. Preserve logical texture sizes, rebuild resized mip chains, retain
  codecs, verify hashes and unchanged entries, and gate writes on savings.
  Ultra Performance uses a 640-pixel maximum edge and 4-bit-equivalent RGB;
  Godot 3 PCM uses 11,025 Hz / 8-bit. Native remains unchanged.
- Add independent Godot 3/4 pack verification and measured library-trial
  reports. Brotato passed manual playtesting; Godot 4 still needs per-game
  tests. Godot 4 audio, embedded-pack replacement and unknown encodings are
  not automatically rewritten. Packed transforms remain explicit exports,
  separate from `--apply-assets`; no game launch or automatic replacement.
- Require Rust 1.89+ to match the pinned MP3 decoder's minimum version.
- Add experimental export-only native UnityFS v6–8 LZ4/HC recompression and
  Godot PCK v1–4 duplicate-resource sharing. Verify every finished output's
  decoded blocks/resource bytes; refuse overwrites, unknown layouts and low
  logical gain/output-write efficiency before writing. No installed auto-apply.
- Add `--audit-container FILE`, `--export-unityfs FILE OUTPUT` and
  `--export-godot FILE OUTPUT`. Godot inventory includes resource types and
  GST2 texture encoding/format/dimension/mip statistics; Unreal Pak inspection
  reports footer version, index encryption and declared codecs (not entry usage).
- Add experimental export-only Godot 3 (PCK v1) packed-audio rewriting:
  `--audit-godot3-audio PROFILE FILE` and
  `--export-godot3-audio PROFILE FILE OUTPUT`. `AudioStreamMP3` (`.mp3str`)
  resources are decoded with a pure-Rust decoder and re-encoded as Ogg Vorbis
  (`vorbis_rs`) under the same entry name, and the matching `.import` stub is
  retyped to `AudioStreamOGGVorbis`. Other entries are copied byte for byte and
  byte-compared. Lossy, profile-gated, export-only. On Brotato this takes the
  pack from 147.9 MB to 88.6 MB (ultra-performance) or 101.4 MB (balanced).
- Add experimental export-only Godot PCK texture downscaling:
  `--audit-godot-textures PROFILE FILE` and
  `--export-godot-textures PROFILE FILE OUTPUT`. Supported `.ctex` (GST2)
  textures (WebP/PNG and BC1/BC2/BC3/BC7, via bundled `image_dds`/`bcdec_rs`/
  `intel_tex_2`) are resized to the profile's longest edge and re-encoded in the
  same format; unsupported encodings and already-small textures are copied
  unchanged. The pack is rebuilt with relocated offsets and recomputed MD5, then
  re-parsed and byte-compared for untouched entries. Lossy, profile-gated,
  export-only; it never overwrites the installed pack and is not auto-applied.
- Correct FMOD/Wwise/CRI classification and distinguish Godot `GDPC` packs
  from Wwise `AKPK` packs. Include extensionless UnityFS bundles, `.unity3d`
  bundles and Amplify virtual-texture containers in engine inventories.

- Add a `keep_languages` setting, a Settings-menu entry and `--keep-languages
  LIST`, letting `--slim` remove language packs the user did not select
  (localized audio, subtitles, `locale/` data) in any engine, whether packaged
  as a language folder or as language-named files (`voiceover_fr.bundle`).
  Groups are skipped unless a kept language is present, so a language set is
  never emptied. Across a 329-game library this finds removable language data in
   59 titles (heuristic candidates, not proven safely removable resources).
- Add `--engines GAME`, a read-only report of the game engine (Unity, Unreal,
  Godot, RE Engine, GameMaker) and its largest containers classified by kind.
  It is the foundation for the format-aware passes and never changes anything.
- Make the skip rule cost-aware. Compression is a rewrite of the whole game, so
  it is gated by the ratio (`min_gain_pct`) only, never by a fixed MiB number
  that would be large for a 1 GB game and trivial for a 40 GB one. Deleting is
  gated separately by `min_prune_mib` (default 1), because deleting is nearly
  free: a 1% saving on a huge game is taken when it needs no rewrite.
- Add `--slim GAME`: one offline, reversible pass that keeps the highest
  resolution tier and the host-platform build, removing other resolution
  fallbacks, non-host platform folders and debug symbols. It is format-agnostic,
  uses no runtime component and never touches groups it cannot classify
  unambiguously. Validated across 329 installed games.
- Add `--variants GAME`, a read-only, format-agnostic detector for redundant
  asset suites (architecture/renderer builds, resolution tiers, platform folders
  and language packs) that share a file-name layout. It suppresses sequential
  content folders that merely reuse names, so it is safe to run on any game.
- Add opt-in `--prune-assets` and `--prune-fallbacks` to move unused developer
  debug symbols, and optionally low-resolution/BC3 fallback suites, into the
  restorable backup tree. Restore/finalize already apply. Backups now fall back
  to a real copy when the filesystem does not support reflinks.
- Require fresh backups and exact physical measurements for shell Lossless
  applies; automatically restore originals if live Btrfs storage does not
  improve. Lossless package size reductions can worsen filesystem compression.
- Add conservative/balanced FMOD export profiles, sparse aligned decoded-PCM
  quality rejection guards, and minimum per-stream savings including seek
  metadata. Add a disposable-copy Btrfs level benchmark for developers.
- Add experimental native FMOD Vorbis transcoding through `--export-fmod`,
  rebuilding FSB5 v1/RIFF-FEV banks while preserving sample ordering, playback
  rates, declared durations, identity and non-codec metadata. Rebuild seek data,
  trim decoder padding, and export only to new files; automatic replacement is
  disabled pending in-game validation. Bink 2 encoding remains unimplemented.
- Add lossless Hades v7 LZ4 package recompression with a statically bundled HC
  encoder, per-block round-trip verification, original chunk boundaries and
  unchanged manifests. Add a Lossless-only asset profile and package counts.
- Keep Bink re-encoding unimplemented and FMOD exports separate from automatic
  asset optimization: detecting containers is not counted as optimization.
- Add an optional visual target setting and a native raster resizer for loose
  PNG, JPEG, WebP, BMP and TGA assets, with preview, measured sizes and restore.
- Process asset previews and changes one file at a time, and stream restore
  checksum verification to avoid game-sized memory usage.
- Inventory PKG/XNB bundles and Bink/video files, report scanned logical bytes
  even when no assets are eligible, and label live disk measurements as excluding
  restore copies rather than implying net disk-space savings.
- Preserve unrecognized restore-directory contents and pre-existing temporary
  files on errors; reject restore paths through symlinked directories or mounts.
- Reject invalid asset actions and oversized DDS headers; propagate interrupted
  WAV resampling instead of treating it as an unsupported file.
- Expose the system-default or explicit ZSTD level in TUI settings; compression
  still runs once before deduplication.
- Add `--recheck` to refresh current file-size and extent measurements without recompression.
- Track dedupe completion and show `COMPACTED` only after both compression and dedupe.
- Batch and selection actions deduplicate compressed-only games without recompressing.
- Base saved compression statistics on logical game-file size, not prior compressed extents.
- Report this pass’s compression change separately from total savings versus original files.

- Bundle a static Rust backend with codecs linked into the executable; no image
  programs or runtime codec packages are invoked.
- Use Linux ioctls for ZSTD compression, extent measurement, deduplication and balance.
- Remove runtime requirements for btrfs-progs, compsize and duperemove.
- Bound dedupe memory using sorted temporary indexes; never submit ranges past EOF.
- Preserve paths and contents, reject symlinks and nested mounts, skip holes, and clean
  temporary state on errors and handled signals. Kernel comparison verifies hashes.
- Stream progress, propagate interruption, fix selected-game titles and distinguish
  logical sharing from physical storage savings.
- Keep per-file output behind `--verbose`; default compression progress is time-limited.
- Fix exact-byte measurement dispatch and use one disk baseline for per-step and total
  savings. Report unknown measurements and negative savings honestly.
- Keep physical dedupe history separate from legacy logical-sharing estimates.
- Package architecture-specific releases and install/update/uninstall both components.


All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Optional Btrfs extent deduplication with `duperemove` after a game is
  compressed, using a persistent hashfile per Steam `common` directory.
- `--dedupe` to run a measured pass over discovered libraries even when games
  are already compressed; it reports before/after Btrfs usage and newly shared
  bytes, and skips libraries with a running game.
- Append-only dedupe measurement history, summarized separately from ZSTD
  compression savings by `--history` and `--stats`.
- Per-library `flock` protection so concurrent processes cannot use one
  duperemove hashfile at the same time.
- Single and batch compression deduplicate each game immediately after its
  defragmentation, using a shared hashfile per library. Compression summaries
  and history label ZSTD savings separately from dedupe's newly shared bytes.
- A project goal to measure real compression plus deduplication savings, inspired
  by the reported Helldivers 2 package reduction while making clear that it is
  not a generic expected result.

### Fixed

- Canonicalized game paths with trailing slashes so duplicate state records and
  incorrect parent-library hashfile keys are not created for one install under
  two path spellings.

## [0.1.1] - 2026-09-28

The version advertised in the README: every feature described there is now in the
released archive. It is a superset of 0.1.0, which was tagged before the ratio
work and the packaging cleanup landed.

### Note for users who installed before this release

The repository owner changed from `pablogonz12` to `pgm1207`. A copy installed
before then has the old path compiled into its self-update, so it still follows the
old URL. GitHub redirects that to the same repository today, so updates keep
working — but if the old username were ever reused by someone else, the redirect
would stop. To repoint an older install, run the installer again from the current
URL (or `--self-update`), which rewrites the copy in place:

```sh
curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh | sh
```

### Added

- A **community ratio table** of 225 measured games (`ratios/games.json`,
  browsable as `GAMES.md` and embedded in the README), with `--ratios`,
  `--export-ratios`, `--import-ratios` and `--render-ratios`.
- A one-command contribution path: `make ratios-merge FILE=…` (sample-weighted
  merge), a "Submit compression ratios" issue form, and `--submit-ratios` to open
  the issue through the user's own `gh` login. Nothing is ever uploaded silently.
- **Self-update**: `--check-update`, `--self-update`, and a throttled once-a-day
  automatic check, all checksum-verified and inert for package-managed installs.
- `--uninstall` (with `--yes`) to remove the tool, manpage, ratio table and
  systemd units while keeping the user's history.
- Community hints in `--status`/`--notify` at the matching zstd level, plus the
  table-driven "likely low yield" advisory.
- The one-line installer now installs the manpage and the ratio table, follows the
  latest release when no version is pinned, detects the distribution, and offers
  to install missing dependencies and to add itself to `PATH`.

### Fixed

- Upgrading no longer discards compression history; state is versioned and
  migrated, and written atomically with a `.bak` backup.
- `compress-force=` mounts, including SteamOS's `compress-force=zstd:6`, are
  recognised; defragmentation mirrors the mount's zstd level via `-L`.
- A game with a live process is never defragmented, re-checked immediately before
  every write.
- `--history` and `--benchmark` no longer rely on gawk-only `asort()`/`strftime()`,
  so they work with mawk on Debian/Ubuntu and under busybox awk.
- Without `compsize`, a compressed game is remembered as compressed (zeroed
  marker) instead of being re-done every run, and stays out of the statistics.
- The TUI no longer prints `local: can only be used in a function` over the
  interface, and its batch honours the low-yield thresholds.
- Many other correctness and safety fixes; see the 0.1.0 entry.

## [0.1.0] - 2026-09-28

The first public release. Everything before it was an internal script that was
never published.

### Added

- Interactive TUI: paginated game list, cursor navigation, multi-select with
  `Space`, select-page with `a`, search with `f`, pending-only filter with `v`.
- Batch compression of every pending game (`b`), with a progress bar and a
  per-game before/after summary.
- Compression statistics view (`t`) aggregating total original size, on-disk size
  and space saved across the whole library.
- Settings screen (`s`): eco mode (`nice`/`ionice`) and custom library management.
- Non-interactive modes for scripting and CI: `--status`, `--dry-run`,
  `--library`, `--pending`, `--no-color`, `--help`, `--version`.
- `--status --json` for script-friendly library, game, and summary data.
- `--history` to print every recorded compression, newest first, and `--stats`
  for the total space reclaimed — reading the state database without opening it
  by hand.
- `--benchmark` to measure real, per-game savings from the recorded `compsize`
  data and report the best game, the overall ratio, and how many games save over
  50%. It is the honest form of the marketing claim: every number is produced
  from actual disks and is reproducible, not estimated.
- A **community ratio table** of ~225 measured games in `ratios/games.json`,
  browsable as `GAMES.md` (regenerated with `make ratios-doc`), with
  `--ratios [GAME]` to look a game up, `--export-ratios` to emit this machine's
  measurements as a document ready for a pull request, and `--import-ratios FILE`
  to use a contributed table as a fallback hint. Every row names the zstd level it
  was measured at, because a game's ratio is a property of *(game, level)* and
  mixing levels would produce confident, wrong predictions. The table is a hint
  only: a local `--benchmark` always wins, it never decides whether a game is
  compressed, and it never triggers a write. `--status` and `--notify` show it
  next to an unmeasured pending game only when the library's mount has the same
  level, labelled as a table figure; `--status --json` carries
  `community_hint_pct`, `community_hint_level` and `community_hint_low_yield`.
  A first-pass game the table expects to save under `min_gain_pct` is flagged
  `likely low yield` in `--status` and `--dry-run` — an advisory only, never held
  back, because only a local pass can produce a trustworthy number. The table is
  installed alongside the tool, so `--ratios` works offline.
- A **contribution path** for the community table: `ratios/merge.py` and
  `make ratios-merge FILE=my-ratios.json` fold an `--export-ratios` document into
  `ratios/games.json` with sample-weighted averaging, sample-count summing and
  min/max widening, then regenerate `GAMES.md` and the README list. A "Submit
  compression ratios" issue form covers people without a checkout. Nothing is
  ever uploaded automatically.
- `--submit-ratios` to offer your measurements back in one command: it shows
  exactly what will be sent (the same document `--export-ratios` produces — no
  paths or host details), asks once, and opens a GitHub issue through the user's
  own `gh` login. A maintainer merges it, so the shared table cannot be poisoned
  by an unreviewed client, and nothing is sent without confirmation.
- `--notify` for optional read-only desktop notifications about actionable games.
- A **watch-only** systemd user timer (`systemd/`, installed with `make service`)
  that runs `--notify` twice a day. It reads the library and sends a notification;
  it never compresses anything. Automatic compression stays a non-goal, because a
  root daemon rewriting a Steam library unattended has to be right every time and
  the failure modes (a crashed game, a flat battery on a handheld) are not
  cosmetic.
- `--balance` as an explicit, opt-in companion step. The canonical SteamOS
  guidance pairs defragmentation with `btrfs balance start -m`, which evens out a
  degraded Btrfs allocation across an SSD — a real part of reclaiming that space,
  and one with enough blast radius that it should never be implied by running a
  defragment pass. `--balance` lists each mount it would touch, explains what the
  command is for, and prints the exact invocation, then exits: it refuses to run
  unattended or without a `y` at a terminal.
- A game with a live process is never defragmented. A game being played while its
  files are rewritten underneath it is the one failure that is not merely wasted
  time, so detection reads `/proc/<pid>/maps` and matches mapped file paths
  against each library root. This works for native and Proton titles alike and
  does not rely on Steam's undocumented `StateFlags`. Such a game is reported as
  `RUNNING`, counted separately in `--status`, and skipped by `--dry-run` and the
  TUI with the reason shown. The check runs again immediately before the write,
  so a game launched part-way through a long batch is still caught.
- `LOW YIELD` games are not re-compressed on every update. A defragmentation pass
  has to push every byte of a game through zstd, so its cost scales with the
  game's size — and so does its benefit, multiplied by how well that game
  actually compresses. Benefit per unit of work is therefore just the game's
  measured compression ratio, and it is independent of the game's size: a 100 GB
  game that only shrinks to 98 GB is not worth an evening, but neither is a
  200 MB one. An updated game whose previous saving was under `min_gain_pct`
  (default **5%**) or `min_gain_mib` (default **32 MiB**) is held back, with the
  predicted saving shown. Either threshold set to `0` disables that half of the
  test, and a game with no history is never held back.
- `--status` always reports skipped empty installs, so nothing is hidden silently.
- Startup preflight: verifies required tools and warns when the library's mount
  point lacks a `compress=` option, with the exact `mount -o remount` to fix it.
- Discovery of Steam libraries from `libraryfolders.vdf` (native, `~/.steam` and
  Flatpak layouts) plus user-registered libraries.
- Compression history keyed by install path, so separate copies of the same game
  in different libraries keep independent status and savings data.
- Reproducible release tarballs, SHA256SUMS, a tag-triggered GitHub release
  workflow, and an installer that verifies the archive before installing.

### Performance

- `--status` and `--dry-run` are about **2.1x faster** on a 225-game, 247k-file
  library (2.63s to 1.25s), by removing work rather than adding machinery:
  - The size column no longer shells out to `du -sh` per game, which re-walked
    every tree that `scan_game_dir` had already walked to compute the very same
    number.
  - `scan_game_dir` now also emits a human-readable size, produced by the `awk`
    that is already running instead of forking one more process per game.
  - `basename` is replaced with pure parameter expansion.
- The size column now reports **apparent size** (sum of file bytes) rather than
  disk usage. On a compressed filesystem the two differ, and apparent size is the
  stable, comparable figure; disk usage for a game is what `compsize` reports in
  the statistics view.

### Fixed

- `compress-force=` mounts were not recognised. Btrfs spells the option
  `compress-force` with a hyphen; the check meant to catch a non-zstd compressor
  looked for `compr_force` with an underscore, so that branch was dead code.
  Worse, the primary check only matched `compress=zstd`, so on a filesystem
  mounted `compress-force=zstd` the tool reported "no compress= mount option" and
  advised remounting with `compress=zstd`. SteamOS mounts `/home` and btrfs SD
  cards with `compress-force=zstd:6` by default, so this fired a false warning on
  the primary target platform.
- Long game names wrapped the list layout. Names are now clamped to their column
  width in the TUI, in `--status`, in `--dry-run` and in the statistics view,
  with the cut made on a UTF-8 character boundary rather than a byte offset.
- **Upgrading silently discarded the whole compression history.** When the state
  key changed from the game name to the install path, every record written by an
  older release was skipped as "not a v2 record", so every game reported as
  `UNCOMPRESSED` and the library had to be re-done from scratch. The state file
  now carries a format-version header, older name-keyed records (both the 8-field
  and the 2-field form) are read back and matched to their install path, and a
  name shared by more than one install is left alone rather than guessed. The
  state file is backed up to `compressed_games.db.bak` before every replacement,
  so a bad write can never cost the user their history again.
- The legacy arrays used during that migration were not declared associative, so
  bash evaluated the string subscript arithmetically and collapsed every game
  into index `0`; the last record then appeared to be every game's history. They
  are now `declare -A`.
- `--history` and `--benchmark` originally used `asort()` and `strftime()`, which
  are gawk extensions. Debian and Ubuntu ship **mawk** as `/usr/bin/awk`, so both
  commands would have died there with "function asort never defined" on a stock
  install. Date formatting is now computed from the epoch with a portable
  civil-from-days routine, and sorting is done by emitting a fixed-width key and
  piping through `sort`. CI now runs the whole suite under both `mawk` and
  `busybox awk` so this cannot regress.
- `--benchmark` sorted its rows with `asort()`, which compares strings: a game
  saving `7.9%` was listed above one saving `68.0%`. Rows are now sorted on a
  zero-padded numeric key.
- `--render-ratios` (and therefore `make ratios-doc`) rendered the table in use,
  which is the user's imported `ratios_hint.json` when one exists. A maintainer
  who had ever run `--import-ratios` would regenerate `GAMES.md` from their
  personal table instead of `ratios/games.json`. The renderer now always reads
  the shipped file, and its "no table" message no longer prints a literal `\n`.
- Steam's empty install stubs — directories left behind by failed or interrupted
  downloads, holding no files or a single 53-byte depot marker — were listed as
  games and reported as `COMPRESSED`. They are now skipped, which also stops them
  inflating the game count. The threshold is configurable via `min_size_mib`
  (default 1 MiB, `0` disables). On one 235-entry library this removed 10 phantom
  entries.
- Pressing `Ctrl+D`, or reaching EOF on stdin, could trigger compression of the
  highlighted game. Input now distinguishes "no key" from a real key, and
  interactive mode refuses to start without a TTY.
- Arrow keys were dropped on slow terminals or under load: the escape sequence
  parser waited only 50 ms for the remaining bytes, so `ESC [ A` was consumed as
  three separate keys. The timeout is now 250 ms per byte and the sequence is
  validated before use.
- Updates written deep inside a game directory were never noticed, because change
  detection only looked 3 levels deep. Detection is now a full recursive
  `find -newermt` with early exit.
- Checkbox selections were keyed by list index, so changing the library list
  (adding or removing a library in Settings) could shift every selection onto a
  different game and compress the wrong one. Selections are now keyed by path.
- Game names containing `sed` metacharacters (e.g. `Warframe [DLC]`) could never
  have their state record replaced, accumulating duplicates and double-counting
  statistics. State writes are now exact-match and atomic.
- With `btrfs-compsize` missing or failing, every game was silently recorded as
  `0 MB` saved and the statistics were permanently polluted. Savings are now
  reported as `n/a`, and an unmeasurable result is recorded as a zeroed marker:
  the game is remembered as compressed (so it is not re-done on every run) but is
  excluded from `--stats`, `--history`, `--benchmark` and `--export-ratios`.
- Savings percentages were recomputed by integer arithmetic from rounded
  megabyte values, losing precision. They are now computed as a float.
- `sudo` is no longer required when running as root, and `sudo -n` is used
  elsewhere so a batch run can never block on a password prompt mid-operation.
- Sizes were parsed with a locale-dependent decimal separator, so a `de_DE` or
  `es_ES` system read `1,4G` as `1`. The C locale is now forced.
- `to_mib` only detected units from the first whitespace-separated field, so a
  space-separated `1.5 GiB` parsed as zero.
- The interactive TUI printed `local: can only be used in a function` over the
  interface on every redraw, because `local tui_pct` was used in top-level code.
  A source-hygiene test now guards the main loop against a stray `local`.
- The TUI batch compressed games held back by `min_gain_pct` whenever
  `min_gain_mib` happened to be `0`, contradicting `--status` and `--dry-run`.
  Low yield is now disabled only when *both* thresholds are zero, skipped
  low-yield games are reported in the batch header, and a batch with nothing left
  to do says so instead of claiming everything is compressed.
- Removing a custom library from Settings could delete the wrong line when the
  file contained blank lines: the menu numbered with `nl` (which skips blanks)
  while removal used the real line number. The menu now numbers every line, and
  `0` is rejected rather than passed to `sed`.
- The interactive prompts (`Search`, `Paste library path`, `Enter number to
  remove`) used `read` without `-r`, so a backslash in a pasted path or filter
  was silently eaten.
- The installer offered `btrfs-compsize` on Arch, Fedora and openSUSE, where the
  package is named `compsize`; only Debian and Ubuntu package it as
  `btrfs-compsize`. The offered command would have failed on those systems.
- `--ratios` (and `--render-ratios`) could not find the bundled table when the
  executable was reached through a symlink (a package manager's `bin` pointing
  into a versioned directory, a Nix profile, a `/usr/bin` alternative). The
  script now resolves its own path before looking for the table, so symlinked
  installs get `--ratios` offline.

### Changed

- Interface strings unified to English.
- `--status` now opens with a header showing the library count and a progress
  bar, colour-codes each game's state, and ends with a one-line verdict when the
  library is fully compressed. The TUI header shows the same progress bar.
- The size check and the modification check now share one `find` pass per game
  instead of two separate walks.
- Dependency check is split: `btrfs` is required only to defragment, so `--status`
  and `--dry-run` work on a machine without `btrfs-progs`.
- The state database is written atomically via a temporary file and `mv`.
- An unmatched keypress is a no-op instead of falling through to a default action.
- Defragmentation now honours the mount's zstd level through `-L` instead of
  always using btrfs's default of 3. On a `compress-force=zstd:6` SteamOS mount a
  library was previously being rewritten at level 3 while all new data was
  written at level 6, giving a mixed and misleading ratio. Overridable with the
  new `compress_level` config (`auto` by default, or 1-15 to pin a level).
- `--status` reports the mount point, its zstd level, the level that will be
  used, and whether a library sits on removable media.
- An empty or absent `min_size_mib` file no longer produces a spurious warning;
  only a present-but-invalid value is reported.
- `--status` and `--dry-run` never modify a game's data or the state file, though
  they do create the config directory and its default files on first run.
- The one-line installer now detects the distribution (`apt`, `dnf`, `pacman`,
  `zypper`), reports missing `btrfs-progs`/`compsize` with the right command for
  that distro and offers to run it on request, and offers to add the install
  directory to `PATH` in the user's shell startup file instead of only printing a
  warning. Nothing is installed or edited without an explicit `y` at a terminal.
- The one-line installer now follows the **latest** release when
  `BTRFS_GAME_COMPRESSOR_VERSION` is unset (it resolves the tag from the GitHub
  release API). Re-running the same `curl | sh` command is therefore an update;
  before, the version was hard-coded and re-running reinstalled the old copy,
  contradicting the README's "latest release" default.
- **Self-update**, reversing the earlier non-goal: `--check-update` reports
  whether a newer release exists, `--self-update` installs it, and a throttled
  once-a-day check (disabled with the `auto_update` config, also run by the
  `--notify` timer) keeps the one-line-installer copy current without a GitHub
  visit. Every update verifies the release `SHA256SUMS` before replacing the
  script; a mismatch is refused and the running file is left alone. A copy owned
  by pacman, Homebrew or Nix is never touched — the user is sent to their package
  manager — and only a user-writable install updates itself. The running process
  is never re-executed, so an update lands on the next run.
- `--uninstall` (and `--uninstall --yes` to skip the prompt) to remove the tool,
  its manpage, the ratio table installed beside it, and the optional systemd user
  units. It asks at a terminal, refuses when a package manager owns the copy, and
  keeps the user's configuration and compression history.
- The one-line installer now installs the manpage (`share/man/man1`) and the
  community ratio table (`share/btrfs-game-compressor`) next to the tool, so
  `--ratios` works offline on an installed copy and `--uninstall` can remove
  everything it put down.
- `--ratios`, `--export-ratios`, `--import-ratios`, `--render-ratios`,
  `--history`, `--stats` and `--benchmark` no longer discover libraries first.
  Each of them used to run a full walk of the game library (and `check_compsize`)
  before doing anything, so `make ratios-doc` in CI and `--ratios` on a machine
  with no Steam library paid for a scan they never needed. They now read the state
  file and the ratio table directly.

### Removed

- `--all-libraries`, which was accepted and documented but never implemented. A
  flag that silently does nothing is a bug, and is now rejected as an unknown
  option.

[0.1.1]: https://github.com/pgm1207/btrfs-game-compressor/releases/tag/v0.1.1
[0.1.0]: https://github.com/pgm1207/btrfs-game-compressor/releases/tag/v0.1.0
