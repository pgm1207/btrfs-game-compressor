# btrfs-game-compressor

**Reduce the disk footprint of Steam games on Btrfs.** Compression and
deduplication keep game files byte for byte intact; an optional, experimental
texture stage can additionally downscale loose and Godot textures, which is lossy
and not verified in-game. Actual savings depend on the game — unusually
compressible titles can save much more than games built from compressed assets.

[![CI](https://github.com/pgm1207/btrfs-game-compressor/actions/workflows/ci.yml/badge.svg)](https://github.com/pgm1207/btrfs-game-compressor/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-any%20Linux%20with%20Btrfs-informational)](#requirements)
[![Shell](https://img.shields.io/badge/shell-bash%204%2B-4EAA25)](#requirements)

**Engine coverage and future work:** [support matrix](SUPPORT.md) (what is
tested, beta, audit-only or unsupported) and [roadmap](ROADMAP.md). Native
filesystem compression is stable and engine-independent; engine asset writers are
**beta** and unverified at runtime; Unity/Unreal packed texture writers are not
implemented and are documented as gaps rather than promised.

If you game on Linux with a Btrfs filesystem — a Steam Deck, an Arch/CachyOS box, a
Fedora or Ubuntu desktop, a Bazzite handheld — there is a good chance a large part of
your library is sitting on disk **uncompressed**, even though compression is switched
on. This tool finds those games, compresses them with ZSTD, and remembers what it has
done so it never repeats work.

It is a Bash interface with a bundled, statically linked Rust backend. No daemon, no root service, no telemetry, no config files
outside your home. It tells you what it is going to do before it does it, never
touches a game that is running, and knows when re-compressing is not worth your time.

> **225 games already measured.** Browse the [full game list](GAMES.md) — best so far
> is `MechHavoc` at **90%** reclaimed, with [57 of 225 saving over
> 50%](#best-measured-games). Your own numbers come from `--benchmark`.

```console
$ btrfs-game-compressor --status
================================================================================
  BTRFS GAME COMPRESSOR v0.2.1   225 game(s) across 1 library(ies)
  [########################....] 92% compacted
================================================================================
GAME                                     STATUS               SIZE EXPECTED GAIN
--------------------------------------------------------------------------------
Hades                                    COMPACTED           11.2G
Caves of Qud                             UPDATED             1.8G
Factorio                                 UNCOMPRESSED        2.1G  table says ~44% (zstd1)
--------------------------------------------------------------------------------
libraries: 1   games: 225   pending: 12   compacted: 209   compressed only: 4   low-yield: 0
```

## Table of contents

- [What it does](#what-it-does)
- [Texture compression](#texture-compression)
- [Quick start](#quick-start)
- [The problem this solves](#the-problem-this-solves)
- [Measured results](#measured-results)
- [Is this for me?](#is-this-for-me)
- [What it does *not* do](#what-it-does-not-do)
- [What makes it safe](#what-makes-it-safe)
- [Requirements](#requirements)
- [Supported platforms](#supported-platforms)
- [Install, update, uninstall](#install-update-uninstall)
- [Usage](#usage)
- [How it decides what to compress](#how-it-decides-what-to-compress)
- [The game list and ratio table](#the-game-list-and-ratio-table)
- [Files](#files)
- [Eco mode](#eco-mode)
- [Troubleshooting](#troubleshooting)
- [Caveats](#caveats)
- [Contributing](#contributing)
- [License](#license)

## What it does

Run `./btrfs-game-compressor --compact-all-no-backup` for a checkpointed installed-library
Balanced asset → Zstd → dedupe pipeline. Python 3 is required. This is irreversible:
no new asset backups are retained. Results and `stats.md` are stored under
`${XDG_STATE_HOME:-~/.local/state}/btrfs-game-compressor/compact-balanced`.
An interrupted asset stage is flagged for manual recovery, never blindly repeated.
Unsupported formats remain unchanged; completion is not full asset coverage or
proof of game compatibility. The command returns nonzero for failed/skipped games.

**Explicit irreversible backend mode:** `bgc-native assets apply-no-backup balanced 1 GAME_DIR`
applies supported Balanced (1080p) assets without creating persistent restore
copies. It still writes and syncs a temporary replacement before atomic rename.
Existing recovery files are not deleted or overwritten. This mode is not runtime
validation: Steam verification/downloads may be required to restore originals,
and unsupported formats stay untouched. Do not use it on a running game.

**Texture compression is experimental.** See
[Texture compression](#texture-compression) for the commands, supported formats
and honest limits.

The checkpointed library pass is implemented by the optional Python helper
`tools/resume-library-compaction.py`, which `--compact-all-no-backup` invokes. It
checkpoints every stage and game to `<state>/results.json`, writes `stats.md`
plus per-game logs, skips running games and existing recovery data, and never
repeats a completed stage. Its allocated-reference figures are not net physical
savings or playback validation.

- **Finds** Steam libraries on Btrfs, from `libraryfolders.vdf` and Flatpak layouts,
  plus any roots you register.
- **Compresses** games through the Btrfs ZSTD defragmentation ioctl, mirroring
  the mounted ZSTD level by default; settings can choose another level.
- **Deduplicates matching data** in each game immediately after compression,
  using the native backend. Manual library scans also find duplicates between games.
- Runs one configurable ZSTD pass, then deduplicates. `auto` follows the
  filesystem's configured level; settings can select levels 1 through 15.
- Tracks `COMPRESSED` and `COMPACTED` separately. `COMPACTED` means compression
  is current and a successful dedupe pass has run since; the batch action dedupes
  compressed-only games without recompressing them.
- `--assets-all` provides a fast, read-only extension/format inventory for every
  installed game. `--assets-verify-all` runs the real Balanced transform-candidate
  planner for all games without writing assets; it decodes candidates and may
  take substantially longer.
- For a reproducible, sortable read-only scan, save `--status --json` and run
  `python3 tools/asset-opportunity-report.py --status-json status.json --backend
  ./bgc-native --output-dir report`. The report keeps a hash-identified backend
  copy, records individual failures, and ranks estimated **logical** savings.
  It checkpoints each game; pass `--resume` with the same inputs after an
  interruption, or choose a new output directory for a fresh scan.
  It separately audits supported Godot 4 PCK textures because the ordinary
  planner counts those packs without predicting their reduction. Godot 3 PCK
  reductions are still unestimated. Neither estimate is physical space freed or
  game compatibility evidence.
  An optional `--xnb-backend /path/to/development/bgc-native --xnb-max-edge 1024`
  adds a separate, theoretical XNB v5 BC texture section. It requires a local
   `development-audits` build and does not add those bytes to the planner total.
   The local schema-3 draft records XNB header/audit coverage and partial errors
   separately, keeping successful production estimates visible if research fails.
   An incomplete XNB section still yields a nonzero exit. Older schema-2 reports
   are preserved but cannot resume under schema 3; use a new output directory.
   These reporting/publication changes passed their local suites and a real
   Carrion/Cocoon trial; results are in the
   [hardening review](docs/ENGINE_HARDENING_REVIEW_2026_10_08.md).
- **Optionally downscales textures** (experimental, opt-in) for loose images and
  DDS plus Godot 3/4 packed textures, preserving the codec and rebuilding mips.
  See [Texture compression](#texture-compression).
- **Never touches a running game**, **stops on failure**, and is **safe to interrupt**
  at any time.
- Shows one concise progress line by default. Pass `--verbose` to list each file.
- **Tells you when it is not worth it** — a game that only saves a fraction of a
  percent is reported as `LOW YIELD` instead of costing you an hour.

One install for every distribution, and it keeps itself up to date:

```sh
curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh | sh
```

Release archives include `bgc-native` for the selected CPU architecture. No
`btrfs-progs`, `compsize`, `duperemove`, Rust compiler, or external filesystem
script is required on the user's machine. Image codecs are statically linked
into `bgc-native`; no image tool is launched at runtime. Standard Linux shell utilities remain
necessary for the Bash interface.

### What automatic deduplication does

After compression, the native backend scans only the selected game. A manual
`--dedupe` scan covers each library and can share blocks across games. It hashes
filesystem-sector-aligned data (normally 4 KiB), sorts hashes using bounded RAM
and temporary files, then submits matching ranges to `FIDEDUPERANGE`. The kernel
compares the actual bytes before sharing storage. Hash collisions cannot corrupt
file contents. Duplicate blocks inside the same file are included.

Files, paths, hardlinks and logical contents are preserved. Symlinks, special
files, nested mounts, holes and unwritten extents are not scanned. Inline data,
trailing fragments smaller than a sector, and duplicates at incompatible byte
alignments are not deduplicated. No asset bundles are unpacked or rewritten.

Each pass rehashes the selected scope; there is no persistent hash database to
become stale or accidentally include another game. Sorted records require about
32 bytes per scanned sector in temporary storage (about 0.8% of scanned data
on 4 KiB Btrfs, with extra space during merges). Temporary files are removed on
normal completion, errors and Ctrl+C. A forced kill or power loss can leave a
`dedupe/native-*` temporary directory for manual cleanup. Per-library locks
serialize compression-followup and manual dedupe passes. Progress streams to the
terminal and a per-scope log in the state directory.

Deduplication runs after compression because defragmentation can break existing
sharing. Compression remains recorded if the subsequent dedupe pass fails.
Rejected requests are reported as errors; a failed pass is not recorded as a
success. Ctrl+C stops the operation and preserves completed work.

The final report measures unique Btrfs data extents before and after deduplication.
It shows the decrease in the physical game-data footprint by compression, by deduplication, and
overall, using the on-disk size before this run as the baseline. Negative savings mean growth; unavailable measurements are shown as unknown. Old logical-sharing records remain in `dedupe_history.db` and are excluded from the new physical-savings history. These measurements require root permission and exclude filesystem
metadata and RAID replication. Snapshot references and concurrent filesystem
writes can affect how much storage Btrfs can release.

The Helldivers 2 size reduction is motivation, not a promised result. Savings
depend on each game's actual duplicate data; filesystem deduplication cannot
restructure game assets as a developer can.

## Texture compression

The asset stage can downscale textures. It preserves the source codec and
rebuilds a complete mip chain, and it only replaces a file when the rebuilt
texture is **strictly smaller** than the original. This is experimental: the
output is lossy and has not been validated in-game.

**Export one texture (never touches the source):**

```sh
./btrfs-game-compressor --texture-compress 1920 texture.dds out/texture.dds
```

**Export a whole tree, writing only files that actually shrink:**

```sh
./btrfs-game-compressor --texture-compress-tree 1920 assets/ out/assets/
```

**Apply to an installed game (beta, opt-in):**

```sh
./btrfs-game-compressor --apply-assets "GAME"   # keeps a restorable backup
```

`--compact-all-no-backup` runs the same asset stage over the whole library with
no restore copies. The active profile sets the resolution cap (`balanced` is
1080p, `performance` 720p, `ultra-performance` 480p).

| Format | Support |
| --- | --- |
| Loose images | PNG / JPEG / WebP / BMP / TGA / GIF / QOI resize |
| Loose DDS | Legacy BC1/2/3, 32-bit BGRA/RGBA, DX10 BC1/2/3/4/5/7, R8G8B8A8/B8G8R8A8, with mip rebuild |
| Godot 3 `.stex` | Single-level PNG / WebP |
| Godot 4 `.ctex` | GST2 BC1/2/3/7 and raw RGBA8 / RGB8 / L8 / LA8 |
| Refused | Cubemaps, arrays, 3D volumes, BC6H, float/HDR, and small or thin textures |

Unity and Unreal packed textures have **no writer yet**. Bundled UnityFS content
can be inventoried read-only with the native `unityfs-inventory` command (while
`--audit-container` reports a bundle's LZ4 recompression potential). Type trees
were present in sampled Addressables bundles; other bundles can omit them.
The bundle inventory reports metadata, not decoded textures, and no Unity texture
is rewritten. The [next compatibility design](docs/ENGINE_COMPATIBILITY_NEXT.md)
details the prerequisites; it is research, not new supported formats.
Local native-only reader drafts and their limited validation status are documented
in [engine development status](docs/ENGINE_DEVELOPMENT_STATUS.md); they enable no
new optimization routes or supported formats.
Encrypted/signed packs and proprietary codecs are skipped.

**Honest limits:** savings are logical file bytes, not measured physical Btrfs
savings, and a game may depend on exact texture dimensions for UI or data.
Validate one game before applying to a library. Where the big texture bytes are
in practice is documented in `test/engine-results/`
(`godot-texture-opportunity-2026-10-07.md`, `dds-apply-hohokum-2026-10-07.md`).

## Quick start

```sh
# 1. Install (any Linux; SteamOS included)
curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh | sh

# 2. Look, don't touch
btrfs-game-compressor --status

# 3. See exactly what a batch run would do
btrfs-game-compressor --dry-run

# 4. Do it, from the interactive UI
btrfs-game-compressor          # then press b for "batch"
```

Compression runs once, then optional **[BETA] asset optimization**, then one
final deduplication pass so it includes changed files. Settings let you use the
filesystem's ZSTD level (`auto`, the default) or choose a level from 1 to 15.
The beta profiles apply across supported loose media: raster images can be
resized, simple legacy BC1/BC2/BC3 DDS textures can be resized and re-encoded,
and simple mono/stereo integer PCM or 32-bit-float WAV can be reduced to the
profile's sample rate/bit depth (non-native profiles output integer PCM).
For resized 8-bit RGB/RGBA non-JPEG, non-DDS images, profiles quantize RGB
precision according to the shared minimum of 7 bits per channel; alpha is preserved exactly.
Ultra Performance additionally converts PCM to 8-bit with deterministic TPDF
dither; higher quality tiers target up to 16-bit PCM. Channels and original
sample rate are never increased, and unsupported WAV chunks/codecs are skipped.
Hades v7 LZ4 `.pkg` bundles can also be recompressed losslessly with the bundled
LZ4 HC encoder. Only blocks that become smaller are replaced; each replacement
is decoded and byte-compared before use. Chunk boundaries, decoded XNB textures,
resource entries and separate `.pkg_manifest` files are preserved.
Bundled LZ4 notices are in [THIRD_PARTY.md](THIRD_PARTY.md) and available from
`bgc-native --licenses`.
Audio is not converted to Opus. A game that expects WAV cannot generally read
an Opus bitstream in its place: Opus has its own codec and Ogg container
specification ([RFC 6716](https://www.rfc-editor.org/rfc/rfc6716.html),
[RFC 7845](https://www.rfc-editor.org/rfc/rfc7845.html)). Changing only the
extension does not make formats compatible. Profiles are Ultra Performance
(480p usage label), Performance (720p), Balanced (1080p), Quality (1440p), Ultra Quality
(4K), Lossless (Hades packages only), and Native. Native is the default. The
Lossless profile never resizes images or changes WAV samples; it only enables
the lossless Hades package pass. Native makes no beta asset changes. Visual
and audio targets reduce detail/quality; Ultra Performance is the most
aggressive supported loose-media profile. They do not add an upscaler to the
game's renderer. Press `o` to preview a highlighted game and optionally apply the
profile, or use:

| Profile | Longest image edge | RGB precision* | JPEG quality | WAV ceiling |
| --- | ---: | ---: | ---: | --- |
| Ultra Performance | 640 px | 4 bits/channel | 60 | 11.025 kHz, 8-bit PCM |
| Performance | 1,280 px | 6 bits/channel | 80 | 32 kHz, up to 16-bit |
| Balanced | 1,920 px | 7 bits/channel | 86 | 44.1 kHz, up to 16-bit |
| Quality | 2,560 px | 8 bits/channel | 91 | 48 kHz, up to 16-bit |
| Ultra Quality | 3,840 px | 8 bits/channel | 95 | 48 kHz, up to 16-bit |
| Native | unchanged | unchanged | unchanged | unchanged |

\* RGB precision applies only to resized 8-bit RGB/RGBA images using
non-JPEG, non-DDS encoders; alpha is retained. WAV ceilings never upsample or
increase bit depth; lower-depth sources remain at their original depth. Lossless
is a separate package-only profile, not a quality-reduction tier.

Audio quality follows the **asset profile**, not the filesystem Zstd level. A
shared policy now feeds loose WAV, Godot 3 packed PCM/Vorbis, and supported
standalone FMOD FSB5 Vorbis banks through the normal preview/apply pipeline:

| Profile | Vorbis quality | Standalone FMOD waveform rejection floor |
| --- | ---: | ---: |
| Ultra Performance | 0.10 | 20 dB |
| Performance | 0.22 | 20 dB |
| Balanced | 0.35 | 20 dB |
| Quality | 0.50 | 22 dB |
| Ultra Quality | 0.65 | 24 dB |

Native/Lossless never transcode audio. FMOD keeps its original playback rate,
channels, frames, loops and event metadata; it rebuilds packet/seek offsets only
when the codec layout is supported and the sample saves at least 5%. The entire
bank must also shrink. Automatic standalone `.fsb`/`.bank` processing is limited
to 256 MiB per file and uses the same original backups, checksums, compression,
repeat-apply protection and restoration as other loose assets. The waveform
guard is not a perceptual guarantee: listen and playtest before finalizing.
Unknown codecs/codebooks/metadata, embedded Unity `.resource` slices, packed
Unreal audio, loose encoded audio and Godot 4 audio remain unchanged.

All lossy profiles share automatic texture quality safeguards, with no per-game
tuning or semantic guesses required:

- Standalone textures at or below **512 px on the longest edge**, or **64 px
  on the short edge**, are left untouched (including RGB quantization).
- A resize is skipped if its resulting short edge would be below **64 px**.
  Images are never enlarged to meet this floor.
- The profile cap is a **soft target**: texture edges retain at least roughly
  **half their original dimensions** (rounding/block alignment may differ by a
  few pixels). A 4K texture therefore stays at least 2K even in Ultra Performance.
  Known original/logical dimensions anchor this budget, so repeated applies do
  not halve a texture again. Godot 4 layouts without a reliable size reference
  remain untouched.
- Explicit RGB rounding retains **at least 7 bits/channel** on supported
  textures, overriding the more aggressive nominal requests in the table.
- Known atlas/spritesheet names are preserved. Godot PCK transforms also inspect
  plain `AtlasTexture` metadata and `.import` references to protect the actual
  encoded sheet. A large sheet can contain tiny sprites: a 4K sheet must not be
  treated like one large background image.

These conservative rules run in both asset apply and Godot exports. They are
not universal UI detection: unnamed atlases or compressed/unrecognized metadata
may not be identified. Start from original assets when comparing profiles;
already-discarded detail cannot be recovered by changing the profile. Manual
visual testing remains necessary.

For maximum compatibility choose **Native**: filesystem Zstd/dedupe preserves
file bytes and works regardless of the engine. Asset rewriting remains opt-in
and format-specific; unsupported Unity serialized textures, proprietary packs,
encrypted/signed layouts and other unknown assets stay unchanged. A safe skip
is preferable to a guessed rewrite. Display inches are usage guidance, not a
reliable way to infer an asset's on-screen size, and presets never set the game's
rendering resolution. No lossy mode can guarantee visual quality in every game.

```sh
btrfs-game-compressor --assets Hades
btrfs-game-compressor --apply-assets Hades
btrfs-game-compressor --restore-assets Hades
# After launching and checking the game, optionally discard the restore copy:
btrfs-game-compressor --finalize-assets Hades
```

Choose a profile in TUI settings before applying. Files change only after
confirmation, and the game must be closed. Changed loose sources are retained in
`.bgc-assets-backup` until restored or finalized. Supported standalone Godot PCK
packs are rewritten in place without a restore copy; recover them with Steam
"Verify integrity of game files". That loose-file copy remains allocated,
so applying may shrink logical file sizes without increasing free disk space.
Finalizing discards the restore copy; snapshots may still retain blocks. Steam
verification can replace changed assets. Anti-cheat is not detected
automatically. Avoid modified assets in online games with anti-cheat. Packed
Unity/Unreal archives, mipmapped/array/cubemap DDS, multichannel or metadata-rich
WAV, and unsupported encoded audio are inventoried but left unchanged. Each changed file is
then compressed once at the configured ZSTD level. The asset optimizer is beta:
test the game before finalizing its restore copies. Hollow Knight's engine
packages are not rewritten. Hades package recompression preserves decoded
content but does not shrink texture dimensions or optimize embedded Bink/FMOD data.
Preview and apply process one asset at a time rather than buffering all converted
files in memory. Restore/finalize refuse changed assets and symlinked game
subdirectories; cleanup preserves unrecognized files in the restore directory.
Previews report packed/video and audio inventory sizes separately from eligible
reductions. A zero-candidate result means no asset optimization savings, even if
large PKG/XNB bundles, Bink videos or FMOD banks are identified. Live game-data
disk measurements exclude restore copies and are not net free-space savings.

**Container conversion status:** Hades v7 LZ4 PKG recompression is implemented;
arbitrary `.pkg` formats and standalone XNB conversion are not. Bink 1/2 videos
remain unchanged. Supported standalone FMOD FSB5 Vorbis banks are now routed
through the beta asset stage, with codec-aware rebuilding of seek data and
preservation of sample indices, identity bytes, loop/timing metadata and event
references. Embedded streams and unknown layouts are not rewritten. Decoding
or renaming these files to MP4/Opus does not make a game load them. Runtime FMOD
compatibility still requires manual playtesting; synthetic tests are not a
gameplay certification.

The shell's **Lossless** apply workflow requires a fresh backup set and exact
privileged Btrfs measurements. If the live footprint is not strictly smaller,
or the post-apply measurement fails, it restores the originals instead of
accepting a logical-only reduction. Existing backups must first be restored or
explicitly finalized. The native `assets apply` command is a lower-level
diagnostic and does not perform this whole-game physical acceptance check.
Before that final acceptance check, it selectively restores packages whose
individual physical footprint did not improve when the privileged selection
report is available. This avoids letting a few poor candidates outweigh a
useful subset. Counts and logical savings exclude restored candidates.

### Experimental native FMOD apply and exports

`--export-fmod QUALITY FILE OUTPUT` performs real lossy Vorbis re-encoding and
rebuilds supported FSB5 v1 files or single-FSB RIFF/FEV `.bank` containers. All
codecs and FMOD Vorbis setup tables are compiled into the backend; there are no
runtime tools or SDK requirements. Quality is `0.0` through `0.4` (higher values
retain more detail, but do not guarantee a smaller file).

Prefer `conservative` (quality 0.3) or `balanced` (quality 0.2) instead of a
numeric quality when testing quality-sensitive assets. These profiles retain
original streams unless a candidate saves at least 5% including seek metadata,
and a sparse, aligned decoded-waveform comparison passes a 20 dB or 18 dB SNR
guard respectively. Numeric quality settings use a 15 dB rejection guard.
These checks catch severe damage and timing shifts; they do **not** certify
perceptual transparency. Listen to representative dialogue, music and effects.
All these checks run offline, adding no new processing stage during gameplay.

`bgc-native fmod-audit FILE` (also routed by `container-audit` for recognized
FSB5 and RIFF/FEV signatures) runs a bounded offline decoder microbenchmark on
up to eight evenly spaced streams (five seconds per stream, five repetitions).
It reports sampled frames, decoded audio seconds and best decoder wall time
summed across the streams. Compare an original with its export using the same
binary on an otherwise idle machine. This tests the bundled lewton decoder,
not the game's FMOD decoder; it does not certify seeking or gameplay CPU costs.

```sh
btrfs-game-compressor --export-fmod 0.0 /path/to/input.bank /path/to/test-output.bank
btrfs-game-compressor --export-fmod conservative /path/to/input.bank /path/to/conservative.bank
```

For the export command, the source is never replaced and existing output files are never overwritten.
Output is written only if the total file is smaller. Mono/stereo Vorbis streams
use verified FMOD codebooks; the encoder keeps playback sample rates, declared
frame counts, sample ordering, names, identity bytes, loops, markers and bank
event metadata, and rebuilds encoded packet offsets and seek tables. Decoder
padding is not counted as additional audio. Unsupported codecs, versions and
container layouts are rejected; streams with unknown codebooks are retained,
and banks with unknown potentially offset-bearing metadata are left unchanged. Input is
limited to 1 GiB, and individual streams to 30 minutes at 48 kHz equivalent.

**This remains experimental:** tests check decoded timing, metadata and main-pipeline
apply/restore, but Hades in-game playback and seeking have not been validated. Do not
replace installed banks without testing on a disposable game copy. Exports do
not change compression history or represent net disk savings. Bink 2 encoding
remains unimplemented; an MP4 renamed to `.bik` is not a compatible replacement.

### Finding redundant asset variants (any game)

```sh
btrfs-game-compressor --variants GAME   # read-only report
btrfs-game-compressor --slim GAME       # one-shot, reversible removal
```

`--slim` performs a single offline pass that keeps the **highest resolution
tier**, removing candidate resolution fallbacks (plus the debug symbols
`--prune-assets` already handles). **All platform-specific assets are retained:**
Linux can launch either a native build or a Windows build through Proton, and
folder names alone cannot establish the runtime. When a language list is set,
it furthermore removes every **language pack** whose language you did not select — localized audio, subtitles and
`locale/` data — so the game only carries the languages you actually use. This
covers both language **folders** (`locale/fr/`) and language-named **files**
(`voiceover_fr.bundle`, `Dialogue_De.bank`, `CueSheet_VO_Battle_ja.awb`), which
is how many Wwise/FMOD/CriWare titles ship voice data.

Do not treat automatic resolution/language classification as proof of runtime
compatibility. Review the displayed removal list and test the game before
finalizing recovery copies. Platform folders are audit-only for slimming.

```sh
btrfs-game-compressor --keep-languages en,de    # codes or names ("English,German")
btrfs-game-compressor --slim GAME
```

The list is stored in `keep_languages` under the config directory and can also be
edited from the **Settings** menu (`[6] Languages to keep for --slim`). An empty
list disables language pruning entirely. Only groups where **every** member is a
recognized language are touched, and a group is left intact unless at least one
of your languages is present in it, so a language set is never emptied. Keep the
language the game boots with (usually the first/English one): removing it can
prevent startup, which is why the confirmation says so and every removal stays
restorable.
 No runtime service, no per-game configuration and no plugins are
involved: it reads directory shape and names, changes files once, and the game *may* continue to play afterwards, but this is not guaranteed. Everything is written to the same restorable backup
tree, so `--restore-assets` undoes it and `--finalize-assets` frees the space
after you test. Ambiguous groups are never touched.


Read-only, format-agnostic report of sibling suites that share an identical
file-name layout but differ in data: architecture builds (`x86`/`x64`), renderer
or shader suites, resolution tiers (`720p`/`1080p`/`4K`), platform folders
(`Windows`/`PS4`/`Mac`) and language packs (`en`/`fr`/`de`). It compares only
directory shape, so it works on games the tool knows nothing about; groups whose
members do not look like variants (sequential level, region or skin folders,
which often reuse file names) are suppressed. Each group shows the bytes
reclaimable by keeping one member. It changes nothing and needs no privilege;
confirm which member the engine actually loads before removing a suite, then use
`--prune-fallbacks`/`--restore-assets` for a reversible change.

### What is this game made with?

```sh
btrfs-game-compressor --engines GAME
```

Read-only report of the engine (**Unity**, **Unreal**, **Godot**, RE Engine,
GameMaker, …) and where the bytes live — the largest containers, classified
(`unity-stream`, `unity-bundle`, `amplify-vtc2`, `pak`, `unreal-iostore`,
`godot-pck`, `fmod-audio`, `wwise-audio`, `cri-audio`, `bink-video`, …).
Includes extensionless UnityFS bundles. Godot/Wwise `.pck` identification uses
the pack signature, not just the extension. Engine/layout evidence is not a
dependency analysis or a guarantee that resources are optional.

### Experimental packed-container optimization

```sh
btrfs-game-compressor --audit-container /path/to/file
btrfs-game-compressor --export-unityfs original.bundle candidate.bundle
btrfs-game-compressor --export-godot original.pck candidate.pck
btrfs-game-compressor --audit-godot-textures ultra-performance original.pck
btrfs-game-compressor --export-godot-textures balanced original.pck candidate.pck
btrfs-game-compressor --audit-godot3-audio balanced Brotato.pck
btrfs-game-compressor --export-godot3-audio balanced Brotato.pck candidate.pck
```

All code and codecs are compiled into the native helper: no plugin, external
encoder, runtime service or modified engine is required. These paths are **not
enabled for automatic installed-file replacement**:

- **UnityFS v6–8:** stronger lossless LZ4/HC recompression, retaining each block's
  decoded bytes, boundaries, codec flags, resource directory and decoded-data
  hash. Stored blocks remain stored. Only stored/LZ4/LZ4HC metadata is supported;
  LZMA, unknown flags/layouts and files above the current 512 MiB in-memory limit
  are rejected. This does not resize textures or transform standalone `.assets`
  / `.resS` streams. Addressables catalogs may record encoded sizes/hashes or
  CRCs; decode verification alone does not establish catalog/game compatibility.
- **Unity standalone SerializedFile v17–22:** `--audit-container FILE` reports
  engine/file versions, platform, type-tree availability, type/object/external
  counts and per-class object bytes (including Texture2D, AudioClip, Sprite and
  SpriteAtlas). Metadata is limited to 32 MiB; both endian layouts are supported.
  IDs/type references/object extents are checked. A bounded type-tree walker now
  reports supported Texture2D dimensions, numeric format ID, mip count, inline
  bytes and declared external path/offset/size. Unknown trees or stripped schemas
  remain opaque; paths are not followed and external extents are not verified.
   Texture payload decoding and writing are not implemented. Large objects/work
   budgets are skipped explicitly in the inspected/opaque texture summary.
- **XNB v4–6:** `--audit-container FILE` recognizes a bounded XNB header, reports
  target/version/flags/file size and known LZX/LZ4 flag state, and never inspects
  reader IDs or payloads. Directory scans inventory signature-validated `.xnb`
  files but do not infer an XNA/MonoGame/FNA engine from them.
- **Godot standalone PCK v1–4 (plain packs):** share byte-identical resource payloads using
  directory offsets. Stored MD5 values are grouping hints only; candidates are
  compared byte-for-byte and every exported resource is verified against the
  source. Paths, sizes, flags and hashes remain unchanged. Streams with bounded
  buffers; encrypted/sparse packs, removal records, partial overlaps and newer
  layouts are rejected. No new decoder or per-entry Zstd flag is introduced.
- **Unreal Pak:** read-only footer versions 1–11, index bounds, encryption and
  declared codec names. Unencrypted primary indexes up to 256 MiB are read
  through the format's SHA1 check; mismatches fail, encrypted/oversized indexes
  are explicitly skipped. Legacy v1–9 unencrypted indexes additionally
  report method IDs and entry kinds, validate data-header consistency/nonoverlap,
  and check eligible stored payload SHA1 within a 64 MiB total budget. Modern
  v10/v11 unencrypted indexes verify the primary, path-hash and directory-index
  SHA1 values, then classify bounded encoded entries (compression slot, encryption,
  stored/decoded bytes, `.uasset`/`.uexp`/`.ubulk`/Wwise/config/raw-media kind)
  without decompressing them. A hash-matching but unsupported body is reported as
  `UNPARSED`, not as corruption. Companion `.sig` presence and frozen-index flags
  are reported, but signatures and entry payloads are not verified.
  SHA1 is a corruption check, not an authenticity guarantee. This does not decrypt
   or rewrite indexes, independently verify codec semantics, or support IoStore/
  RE Engine repacking. A declared codec is not permission to switch codecs.
- **Unreal IoStore:** content-detected `.utoc` header inventory for TOC versions
  1–8: chunk/block counts, method-table dimensions, partition metadata, security
  flags and a version-aware minimum file-size check. Same-stem regular `.ucas`,
  `.sig` and `.pak` companions are reported using metadata only; links are not
  followed. Chunk tables, signatures, directory contents and `.ucas` payloads
  remain unverified. This is not cooked-texture decoding or repacking.
- **Real-game Ultra Performance trial:** Pathogenic's PCK became 45.4% smaller
  on a copy, with independent unchanged-entry verification and a zero-change
  repeat. The user reports the candidate does not launch; it is on hold and is
  not runtime-compatible. This exposed and fixed large-scene atlas scanning and
  BC block-padding repeat drift. Physical savings remain unverified; see
  [the trial record](test/engine-results/pathogenic-ultra-2026-10-02.md).
- **Carrion read-only/temporary-export trial:** verified XNB headers, inventoried
  FMOD banks and safely tested a disposable `Sounds.bank` export. No installed
  content was changed and no in-game compatibility is claimed; see
  [the Carrion trial record](test/engine-results/carrion-steam-trial-2026-10-03.md).
- **Local XNB development trial:** a feature-gated, detached v5 Texture2D export
  resized seven copied Carrion artbook/comic pages from 2048 to 1024 pixels,
  saving 14,155,776 logical bytes in that sample. The BC1 path retains its
  one-bit alpha mask. An independent parser checked structures, and Pillow
  decoded and compared pixels. Scratch Btrfs copies used 14,155,776 fewer
  allocated bytes; game compatibility and installed-game savings remain
  unverified. This command is
  absent from normal builds and the installed asset pipeline; see
  [the XNB export trial](test/engine-results/carrion-xnb-export-2026-10-07.md).
- **Steam-library Godot Balanced (1080p) copy trial:** exercised the actual
  `assets apply balanced` pipeline on ten disposable installed-game copies
  across Godot PCK v1/v2/v3/v4. Nine were accepted and audited; eligible packs
  shrank and second passes were byte-identical no-ops. PCK v2 remains read-only.
  No installed games changed and no playability claim is made; see the
  [trial record](test/engine-results/godot-balanced-steam-copies-2026-10-03.md).
- `Balanced (1080p)` is a per-format asset profile, not a universal resolution
  switch or engine compatibility promise. It caps supported texture/image edges
  at 1,920 pixels and applies format-specific audio settings; unsupported
  containers, stripped Unity texture schemas and unsupported PCK layouts remain
  untouched. `--asset-inventory GAME` and `--assets-all` are fast, read-only
  extension/format inventories. `--assets GAME` computes decoder-backed
  candidates for one game; `--assets-verify-all` runs that read-only planner for
  the full installed library and may take much longer.
- `--asset-containers GAME` lists standalone PCK/FM0D/Hades-package candidates;
  `--audit-game-packs GAME` performs a read-only PCK structure audit. Neither
  command predicts savings or proves in-game loading.
- The standalone FMOD audit on 2026-10-04 covered 1,444 banks in 59 installs;
  889 matched the strict supported structural parser. Rejections are not rewrite
  candidates; embedded/unknown layouts stay untouched. The regular reversible
  asset pipeline was also tested on disposable copies of eight FMOD-heavy games.
  See the
  [audit record](test/engine-results/fmod-installed-audit-2026-10-04.md).
- PCK read-only audit bounds now accommodate Until Then's 211,483-entry pack;
  unsupported encrypted/sparse PCK layouts are still rejected.
- The 310-install inventory and format-coverage caveats are in the
  [2026-10-04 coverage report](test/engine-results/steam-library-asset-coverage-2026-10-04.md).
- The library-wide coverage snapshot records candidate-scan limits and
  unsupported Unity/Unreal/audio/video formats in
  [the 2026-10-04 report](test/engine-results/steam-library-asset-coverage-2026-10-04.md).
- **Godot PCK textures (lossy, asset apply or export):** `--export-godot-textures PROFILE`
   supports Godot 3 `.stex` (GDST, PCK v1) and Godot 4 `.ctex` (GST2, PCK v3/v4).
   It downscales supported textures to the profile's longest edge
  (640/1,280/1,920/2,560/3,840 px) and re-encodes them in their original format:
   WebP/PNG stay WebP/PNG (lossless encoding of intentionally degraded pixels),
   and BC1/BC2/BC3/BC7 retain their block format. Lower tiers also quantize RGB
   precision; alpha channels are retained. Logical dimensions are preserved,
   including atlas size overrides; resized mip chains are rebuilt completely.
   Unsupported encodings (Basis Universal, ETC, ASTC, half-float) stay unchanged.
    A texture is only rewritten when the new payload is strictly smaller.
    Shared small/thin texture and known-atlas safeguards apply to every tier.
    The normal `--apply-assets` pipeline uses the same transforms for standalone
    packs, without a packed-file backup (Steam verification is the recovery path).
   The pack is rebuilt (32-byte payload alignment for v3/v4), with relocated offsets and
  recomputed per-entry MD5; the export is re-parsed and every untouched entry is
  byte-compared. `--audit-godot-textures PROFILE` is the read-only estimate.
   This is gated to explicit lossy profiles (Native/Lossless never run it).
   Brotato's Godot 3 result has passed manual playtesting. Godot 4 candidates
   still need per-game loading/visual checks; see `test/engine-results/` for
   current trials and physical measurements. For the explicit Slay the Spire 2
   export/Zstd/dedupe workflow, see [the manual Godot trial guide](test/GODOT_MANUAL_TRIAL.md).
- **Godot 3 PCK audio (lossy, asset apply or export):** `--export-godot3-audio PROFILE`
  reads standalone Godot 3 PCK v1 packs, parses the `RSRC` binary resources,
  decodes `AudioStreamMP3` (`.mp3str`) and re-encodes it as Ogg Vorbis under the
  same entry name, retyping that entry's `type=` in the matching `.import` stub
  (Godot 3 identifies a resource by its stored type, not its extension). Every
   supported `AudioStreamSample` PCM is also reduced in rate/bit depth, retaining
   the original resource class and container metadata, with scaled loop points.
   IMA-ADPCM is disabled following a real-game crash. Other entries stay unchanged.
   Profiles map music to Vorbis quality 0.10/0.22/0.35/0.50/
  0.65. `--audit-godot3-audio PROFILE` is the read-only estimate. On Brotato
   (Godot 3.7) the retained music-only result rewrote 34 of 35 tracks: 147.9 MB
   to 88.6 MB. With Ultra Performance PCM8 at 11.025 kHz and GDST textures,
   the manually playtested pack is 64.6 MB logically (55.1 MB of referenced
   physical data); the whole install is 144.8 MB logically / 90.6 MB physically.
   Godot 4 audio resources, QOA and FMOD banks are not covered by this command.

`--audit-container` reads format headers/signatures, not extensions. UnityFS auditing performs
compression/verification in RAM without an output file; Godot auditing reports
resource-type sizes and GST2 texture headers (encoding, numeric GPU format,
dimensions, mipmapped counts). Unreal auditing reads the footer and, within its
budget, verifies unencrypted primary-index bytes and supported legacy directory/
data-header/stored-payload subsets. No cooked textures or audio are rewritten.

Exports refuse existing destinations, including the source itself. Before any
export writes, **logical bytes saved / complete output bytes** must meet
`min_gain_pct` (default 5%). The cost is the touched container, not the whole
game. `0` disables this efficiency gate, not verification. Retaining the source
means an export consumes extra storage; logical reduction is not physical/net
savings. Game loading and real Btrfs footprint still need separate validation.

Additional native research commands:

```sh
bgc-native unityfs-audit FILE
bgc-native godot-dedup-audit FILE    # verified duplicate candidates; no output
bgc-native godot-audit FILE
bgc-native godot-texture-audit PROFILE FILE
bgc-native godot-texture-export PROFILE MIN_PCT INPUT OUTPUT
bgc-native godot3-audit PROFILE FILE
bgc-native godot3-optimize PROFILE MIN_PCT INPUT OUTPUT
bgc-native unreal-audit FILE
```

Recorded experiments and limitations: [engine studies](test/ENGINE_EXPERIMENTS.md).

### Removing unused content

Some installs ship bytes the released game never loads. Two opt-in commands move
them into the same restorable backup tree as every other asset change:

```sh
btrfs-game-compressor --prune-assets Hades       # *.pdb / *.ilk debug symbols
btrfs-game-compressor --prune-fallbacks Hades    # also the unused 720p/BC3 suites
```

`--prune-assets` removes developer debug symbols (`*.pdb`, `*.ilk`) that are
compiled-out data no released build loads. `--prune-fallbacks` additionally
removes low-resolution and BC3 texture/video fallback suites that an engine only
selects when it decides to use low-resolution assets (typically a weak GPU or an
explicit setting). **Only use `--prune-fallbacks` if this machine really renders
the full-resolution assets**; otherwise the game may fail to find them. The
backup makes either change recoverable with `--restore-assets`, and free space
returns only after you test the game and run `--finalize-assets`.

Selection is conservative: only regular files are touched, symlinks and paths
that escape the game directory are rejected, and empty directories are left in
place so restoration never has to recreate them. As with asset transforms, the
backup relies on reflinks where available and falls back to a real copy on other
filesystems.

### Measuring filesystem level tradeoffs

For developer experiments, `python3 test/storage-benchmark.py --scratch NEW_DIR
FILE...` compares ZSTD levels 3, 6 and 9 on independent copies in a new Btrfs
directory. It authenticates through sudo, reports exact physical extents and
encoding time, verifies content hashes and measures repeated warm read+hash
times. Inputs stay unchanged; copies and a JSON report remain for inspection.
Choose representative Bink, package, audio, text and binary inputs. These are
sample-only measurements, not whole-game projections or in-game CPU benchmarks;
the script never drops system caches. Python is only needed for this diagnostic.

`bgc-native package-audit FILE` similarly measures warm LZ4 decoder wall time
for Hades v7 packages (five runs, at most 512 MiB of decoded work per run),
excluding input reads. Compare original and recompressed copies to investigate
decoder cost without modifying the game. It is not a game-loading benchmark.

`python3 test/package-storage.py SOURCE_PACKAGES --scratch NEW_DIR` runs the
full lossless Hades package pass on independent copies, measures physical data
before/after, and reports retained-backup overhead separately. It never applies
to the source directory or discards backups. Both diagnostic scripts accept
`--sudo-stdin` for a credential supplied through standard input; credentials
stay in process memory and are never written to their reports or files.
Add `--select-physical` to the package diagnostic to restore individual copies
whose measured physical footprint fails to improve, then remeasure the selected
set. Native `measure-file FILE` and `assets-restore-file ROOT RELATIVE_PATH` are
lower-level diagnostic interfaces for exact measurement and checksum-validated
selective restoration. Existing retained backups remain recoverable.

There is no universal asset conversion that is both smaller and compatible with
every game. Steam depot manifests record file hashes, and game loaders expect
their own asset formats. Changing asset bytes may trigger a Steam repair or make
the game fail to load. Filesystem compression and deduplication preserve those
bytes. They may reduce disk reads, but this tool does not claim higher FPS or
smoother play without measurements on the specific game and device.
See [Steam's manifest format](https://partner.steamgames.com/doc/store/application/builds),
[Btrfs compression](https://btrfs.readthedocs.io/en/latest/Compression.html), and
[Btrfs defragmentation](https://btrfs.readthedocs.io/en/latest/Defragmentation.html).

Nothing is written to a game until you ask for it. `--status` and `--dry-run` are safe
in scripts and CI.

## The problem this solves

Btrfs compresses on write, so you would expect a `compress=zstd` mount to shrink your
games automatically. It does not, and the reason is specific:

> **Steam preallocates game files with `fallocate`, which creates unwritten extents
> that never get compressed on write.**

So a fresh install sits on disk uncompressed even on a correctly configured
filesystem. A fresh Baldur's Gate 3 install reports 90% uncompressed until you
defragment it — a long-standing, widely reported issue, see
[valvesoftware/steam-for-linux#12974](https://github.com/valvesoftware/steam-for-linux/issues/12974).

This applies to **every** Btrfs setup, not just SteamOS. The only guidance in the wild
is a manual `btrfs filesystem defragment` you have to remember and re-run.

**And it is not a one-time chore.** Every Steam update rewrites files and breaks
compression on them again. This tracks which games have drifted, so you only redo the
work that is actually needed.

## Measured results

Real numbers, not projections. The benchmark comes from 15 games measured with
`btrfs-compsize` on one machine; the rest is the best of the **225-game [community
table](#the-game-list-and-ratio-table)** that ships with the tool.

| | |
|---|---|
| **Whole library reclaimed** (15-game compsize benchmark) | **40.6%** (6.29 GiB → 3.74 GiB) |
| **Best in the benchmark** | **68.0%** — `Astro Prospector`, 338 MiB → 108 MiB |
| **Best in the 225-game community table** | **90.0%** — `MechHavoc`, 2 measurements, 89.9–90.0% |
| **Games saving over 50%** | **57 of 225** measured in the community table |

### Best measured games

A sample of the [full 225-game table](#the-game-list-and-ratio-table) — the tool's own
community measurements. Every row is a real `compsize` saving at zstd level 1;
[`GAMES.md`](GAMES.md) carries the sample count and observed range too.

| Game | Saving | Samples |
|---|---:|---:|
| **MechHavoc** | **90.0%** | 2 |
| Database Detective | 83.6% | 2 |
| Wall World | 83.0% | 2 |
| Forage Wizard | 82.7% | 2 |
| DemonLordJustABlock | 78.6% | 2 |
| Lossless Scaling | 77.8% | 2 |
| P0 | 75.4% | 2 |
| Hollow Knight | 75.0% | 2 |
| Grey Hack | 73.4% | 2 |
| KIDS | 71.2% | 2 |

…and [215 more in `GAMES.md`](GAMES.md). From the terminal, `btrfs-game-compressor
--ratios` browses the list and `--ratios "Factorio"` looks up one game.

```
$ btrfs-game-compressor --benchmark
Astro Prospector          338 MiB ->  108 MiB   68.0%
Away                      123 MiB ->   40 MiB   67.5%
Backpack Hero             617 MiB ->  219 MiB   64.5%
Bitburner                 450 MiB ->  173 MiB   61.6%
Alabaster Dawn            776 MiB ->  348 MiB   55.2%
...
A Short Hike              328 MiB ->  233 MiB   29.0%
Arco                      635 MiB ->  630 MiB    0.8%
--------------------------------------------------------------------------------
  Games measured:        15
  Before:                6.29 GiB
  After:                 3.74 GiB
  Reclaimed:             2.55 GiB (40.6%)
  Best single game:      Astro Prospector (68.0%)
```

Run `btrfs-game-compressor --benchmark` yourself and you will get **your** numbers,
computed from your own disks. Nothing is estimated or hard-coded.

## Is this for me?

Two commands, ten seconds:

```sh
findmnt -no FSTYPE /path/to/your/steamapps/common    # btrfs?
findmnt -no OPTIONS /path/to/your/steamapps/common   # has compress= or compress-force= ?
```

If the first says `btrfs` and the second mentions `compress`, this tool will give you
back real disk space. If it says `ext4`, `xfs` or `ntfs`, Btrfs compression does not
apply to you.

> **Steam Deck / SteamOS users:** the answer is almost always yes. `/home` and Btrfs
> SD cards ship with `compress-force=zstd`, so compression is on and most games are
> still stored uncompressed. This is the tool's primary target.

## What it does *not* do

Being clear about this up front is the point, because a tool that compresses your game
library has to be predictable:

- **It does not shrink every game to 10% of its size.** Btrfs compresses with ZSTD, a
  general-purpose lossless codec, and modern games ship most assets already compressed
  (textures, video, audio). The project's measured games include outliers up to
  **90%** (`MechHavoc`) and duds near **0%** (`Arco`). Anything claiming
  a large saving on *every* game is not describing this tool.
- **Compression and deduplication do not touch saves, mods or Proton prefixes.**
  They preserve bytes under `steamapps/common`, so those operations do not alter
  Steam verification results.
- **The optional experimental texture/asset stage can reduce quality or affect
  compatibility.** It resizes supported loose images and DDS (including DX10
  BC1/2/3/4/5/7 with mip rebuild; BC6H is unsupported) and Godot 3/4 packed
  textures, and reduces simple WAV audio. Unity and Unreal packed textures have
  **no writer** and are left
  untouched. Native is the default; backups remain until you test the game and
  finalize, and the texture stage is not verified in-game.
- **It does not promise faster games.** The measured benefit is disk space. Load
  times and frame rates depend on the game, storage device, and CPU.
- **It does not run unattended.** No background daemon rewrites a library. The optional
  systemd timer only *notifies*.

### A word on performance

Filesystem compression can reduce bytes read from storage and add CPU work to
decompress them. Either can dominate a particular game's load path. This project
has not measured a general change in load times or FPS, so judge runtime effects
on your own device. Higher configured ZSTD levels spend more CPU time during
the single rewrite and do not themselves promise faster play.

## What makes it safe

The reason a tool like this is worth trusting is not the compression — that is one
kernel interface — it is the guard rails around it:

- **A running game is never defragmented.** The tool checks `/proc` for a process that
  has a file from the game's directory mapped, and re-checks immediately before every
  write. No more "Steam verified the game and my save died".
- **It never compresses anything you did not ask for.** No daemon, no background
  writes. The optional systemd timer only *notifies*.
- **It tells you when a game is not worth the time.** A game that only saves 0.3% of
  its size is reported as `LOW YIELD` instead of costing you an hour.
- **It stops on failure.** If native compression fails, the game's saved state is left
  exactly as it was, so the next run retries it cleanly.
- **`--dry-run` truly changes nothing**, and `--status` is safe in scripts and CI.
- **Its own updates are checksum-verified.** See [Install, update,
  uninstall](#install-update-uninstall).

## Requirements

| | |
|---|---|
| **Required** | Bash 4+, the bundled `bgc-native`, awk, coreutils/findutils/util-linux (including `flock`), and the existing terminal utilities |
| **Optional** | `sudo` for privileged measurement/balance; `notify-send` for desktop notifications; curl/tar for installation and updates |
| **Filesystem** | Btrfs on x86_64 or aarch64 Linux; explicit ZSTD compression levels require Linux 6.15+ |
| **Build only** | Rust/Cargo 1.89+ and a C linker; Cargo fetches the pinned image codec crates into the static backend |

No filesystem-tool packages need to be installed. The kernel performs compression
and deduplication. Root permission is needed for exact compressed extent metadata;
without it, compression can still succeed on files you own and savings remain
`n/a`. Mount compression options govern future writes; this tool explicitly
requests compression of existing files regardless of the mount default.

## Supported platforms

The tool works on any Linux with Btrfs. It detects your distribution and reports it in
`--status`, and is tested and tailored for:

| Distro | Notes |
|---|---|
| **SteamOS / Steam Deck** | `compress-force=zstd` detection, SD card awareness |
| **CachyOS** | `compress=zstd:1` mount, zstd level mirroring |
| **Bazzite** | Fedora-based, same mount options as Fedora |
| **Fedora** | `compress=zstd` mount, standard Btrfs setup |
| **Arch Linux** | `compress=zstd` mount, same install path as any other distro |
| **Ubuntu / Debian** | `compress=zstd` mount; kernel version requirement applies |
| **openSUSE** | `compress=zstd` mount; kernel version requirement applies |

Derivatives (Pop!_OS, EndeavourOS, Nobara, etc.) are detected via `ID_LIKE` and inherit
their parent distro's behavior. Anything else says `platform: other` and still works.

On the immutable SteamOS/Steam Deck, use the one-line installer: it installs to
`~/.local/bin` on the persistent `/home` partition, so it survives OS updates — pacman
changes to the read-only `/usr` do not.

### Architecture

The Bash interface is shared by x86_64 and aarch64 Linux. Releases contain a
native static executable built separately on each architecture. The installer
selects the matching archive. x86_64 Btrfs is exercised locally; aarch64 release
builds run in CI, but real aarch64 filesystem behavior still needs validation.

## Install, update, uninstall

There is **one supported install**: the one-line installer, which works on every Linux
distribution and updates itself. Cloning the repo is only for contributing.

### Install

For the 0.1.x → 0.2.0 transition, rerun the installer once: old self-updaters
only replace the Bash file and cannot install the new bundled backend.

```sh
curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh | sh
```

It downloads the latest release archive, verifies it against the release's
`SHA256SUMS`, and installs the script and `bgc-native` to `~/.local/bin`, the manpage to
`~/.local/share/man/man1`, and the community ratio table to
`~/.local/share/btrfs-game-compressor/`. It **refuses to install** if the checksum does
not match. The native executable is prebuilt and statically linked, so no build is needed
on the destination machine — including SteamOS, where `~/.local` is on the persistent `/home`
partition.

If you prefer to read a script before running it (a good habit), download it first:

```sh
curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh -o install.sh
less install.sh          # read it
sh install.sh
```

Options, either as environment variables or by editing the script:

| Variable | Default | Meaning |
|---|---|---|
| `PREFIX` | `$HOME/.local` | install under this prefix (`$PREFIX/bin/`) |
| `BTRFS_GAME_COMPRESSOR_VERSION` | latest release | pin a specific version, e.g. `0.1.1` |

```sh
# install system-wide instead of per-user
PREFIX=/usr/local curl -fsSL .../install.sh | sudo sh
```

If `~/.local/bin` is not already on your `PATH`, the installer offers to add it, or
prints the exact line to add yourself:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc && exec bash
```

### Update

The installed copy updates itself, so there is no GitHub visit and no package manager
required:

```sh
btrfs-game-compressor --check-update   # is a newer release available?
btrfs-game-compressor --self-update    # update in place now
```

On top of those, a newer release is looked for **once a day**, automatically, when you
open the interactive UI or when the watch timer runs `--notify`. Re-running the
one-line installer also updates. Turn the automatic check off with:

```sh
echo 0 > ~/.config/btrfs-game-compressor/auto_update
```

Every update downloads the release archive and verifies it against the release's
`SHA256SUMS` before replacing the script; a mismatch is refused and the running copy is
left untouched. The update takes effect on the next run, never mid-operation. A copy
owned by a package manager (a distro package under `/usr`, Homebrew, Nix) is left
alone: self-update refuses and tells you to update through that package manager
instead, so the two mechanisms never fight.

### Uninstall

```sh
btrfs-game-compressor --uninstall          # asks first
btrfs-game-compressor --uninstall --yes    # no prompt (still refuses package-managed copies)
```

It removes the script, the manpage, the ratio table, and the optional systemd user
units. **Your configuration and history are kept** — `~/.config/btrfs-game-compressor`
and `~/.local/state/btrfs-game-compressor` are left in place for a future reinstall.
Delete those directories by hand for a clean slate. If you installed with
`make install`, use `sudo make uninstall` and `make service-off` instead.

## Usage

Run with no arguments for the interactive UI:

```sh
btrfs-game-compressor
```

### Keys

| Key | Action |
|---|---|
| `↑` `↓` / `k` `j` | move cursor |
| `←` `→` / `h` `l` | previous / next page |
| `Space` | toggle checkbox |
| `a` | toggle all on the current page |
| `Enter` or `c` | compress and deduplicate the highlighted or checked games; compressed-only games get deduplication |
| `b` | process every pending game |
| `o` | preview and optionally apply the configured [BETA] asset profile |
| `f` | search / filter |
| `v` | show only pending games |
| `t` | savings statistics |
| `s` | settings (ZSTD level, [BETA] asset profile, speed and libraries) |
| `q` | quit |

### Non-interactive

Every mode below is non-interactive. `--dedupe` and `--balance` change filesystem
data; `--recheck` refreshes saved measurements without rewriting game files. A game
is `COMPACTED` when its compression is current and a successful dedupe pass has
completed since that compression. `COMPRESSED` games are queued for dedupe only.

```sh
# one-shot report
btrfs-game-compressor --status

# only what still needs compressing
btrfs-game-compressor --status --pending

# what a batch run would do, changing nothing
btrfs-game-compressor --dry-run

# deduplicate all discovered Btrfs Steam libraries and measure the result
btrfs-game-compressor --dedupe

# refresh measured savings without recompressing any files
btrfs-game-compressor --recheck

# register a library outside Steam's libraryfolders.vdf
btrfs-game-compressor --library /mnt/games

# machine-friendly
btrfs-game-compressor --status --no-color | column -t

# JSON for scripts
btrfs-game-compressor --status --json

# Savings by category: ZSTD, dedupe, and tracking status
btrfs-game-compressor --stats

# every recorded compression, newest first
btrfs-game-compressor --history

# measured savings per game, from your own disks
btrfs-game-compressor --benchmark

# expected savings from the community table
btrfs-game-compressor --ratios "Factorio"

# read-only desktop notice (requires notify-send)
btrfs-game-compressor --notify
```

| Flag | |
|---|---|
| `-h, --help` | usage |
| `-V, --version` | version |
| `-s, --status` | non-interactive report, then exit |
| `-n, --dry-run` | list pending work, change nothing |
| `--dedupe` | deduplicate Btrfs Steam libraries and report measured usage before and after |
| `--recheck` | refresh current game size and savings measurements without recompressing |
| `--assets GAME` | preview supported asset changes |
| `--apply-assets GAME` | apply beta optimization to supported images, simple DDS, and WAV audio; keeps a local restore copy; needs a terminal |
| `--restore-assets GAME` | restore original assets |
| `--finalize-assets GAME` | discard restore copy after testing; needs a terminal |
| `-l, --library DIR` | add a library, then exit |
| `--pending` | restrict `--status` / `--dry-run` to pending games |
| `--no-color` | disable ANSI color (also honours `$NO_COLOR`) |
| `--json` | with `--status`, `--benchmark` or `--ratios`, emit machine-readable output |
| `--history` | print separate compression and deduplication measurements |
| `--stats` | print ZSTD savings and latest per-library dedupe estimates |
| `--benchmark` | measure real per-game savings from recorded data |
| `--ratios [GAME]` | look up expected savings in the community table |
| `--export-ratios` | emit your measured ratios as JSON to contribute |
| `--import-ratios FILE` | use a ratios JSON as a fallback hint |
| `--render-ratios` | render the table as Markdown (maintainers) |
| `--check-update` | report whether a newer release exists, change nothing |
| `--self-update` | update this copy in place, verifying the release `SHA256SUMS` |
| `--uninstall` | remove the tool, manpage and systemd units; keeps history |
| `--submit-ratios` | open a GitHub issue offering your measured savings (needs `gh`) |
| `--yes` | with `--uninstall` or `--submit-ratios`, do not ask |
| `--notify` | send a read-only desktop notification for changed games |
| `--balance` | offer to rebalance each library's mount; reports and confirms, never unattended |

## How it decides what to compress

Each game gets one of six states:

- **COMPACTED** — compressed and successfully deduplicated, with no later file changes.
- **COMPRESSED** — compression is current, but deduplication is pending. Batch mode runs dedupe only.
- **UPDATED** — previously compressed, but files changed afterwards (a patch, a save, a
  Proton update). Worth re-compressing.
- **UNCOMPRESSED** — never compressed by this tool.
- **LOW YIELD** — updated, but not expected to be worth the time. Pending dedupe still runs in batch mode.
- **RUNNING** — a live process has a file open inside the game's install directory, so
  it is in use. Held back, and counted separately in `--status`.

### A running game is never touched

The one failure mode that is not merely wasted time is rewriting the files of a game
somebody is playing. A game is treated as running when any process has a file mapped
under its install directory, which is read from `/proc/<pid>/maps` and matched against
each library root — so it works for native and Proton titles alike, and does not depend
on Steam's own `StateFlags`.

That check runs again immediately before the write, not just once at startup, so a game
you launch halfway through a long batch is still caught rather than being rewritten
underneath itself.

### Rebalancing is a separate, deliberate step

`btrfs filesystem defragment` compresses extents; it does not fix a degraded block
group layout, which is what actually returns space on an SSD that has fragmented.
`btrfs balance start -m` does that, and the canonical SteamOS guidance runs it after
defragmenting.

Because it rewrites the whole device rather than a game, this tool never implies it:

```sh
# reports every mount it would touch, prints the exact command, and exits
btrfs-game-compressor --balance

# then, if you agree with what it printed:
sudo btrfs balance start -m /home
```

It refuses to run unattended: no terminal, or anything other than `y`, means it reports
and exits without doing anything.

Change detection is a recursive `find -newermt` over the whole game directory, so
updates buried deep in a game's data tree are still noticed. `Proton*`,
`SteamLinuxRuntime*` and the Steam shared folders are skipped automatically.

### Not every update is worth compressing

Compressing a game has to push every byte of it through zstd, so the cost of a pass
scales with the size of the game. The saving scales with size too — but multiplied by
how well that particular game actually compresses. Which means:

> benefit per unit of work = the game's measured compression ratio

and that ratio is **independent of the game's size**. A 100 GB game that only shrinks to
98 GB is not worth an evening, but neither is a 200 MB one. So the test is a percentage,
not a byte count.

The tool already records how much each game saved the last time it ran, so after your
first pass it knows which games are worth revisiting. A game whose previous saving was
under **5%** of its size, or under **32 MiB** in total, is marked `LOW YIELD` and held
back — reported, but not compressed:

```
libraries: 1   games: 225   pending: 12   compressed: 163   low-yield: 50
MonsterHunterRise      LOW YIELD   34.4G  ~340M expected
Dispatch               LOW YIELD   14.2G  ~120M expected
```

There is exactly one gate, and it is a **percentage**, because the cost scales with
the game: compressing rewrites every byte through zstd, so both the time and the SSD
wear grow with size. A fixed MiB number would be huge for a 1 GB game and trivial for a
40 GB one, so it is deliberately not used here.

```sh
# a game must have saved at least this percentage of its size last time
echo 5 > ~/.config/btrfs-game-compressor/min_gain_pct
```

Lower it to compress more eagerly; `0` disables the gate entirely. A game with no
history is never held back — it has to be compressed once before anything can be
predicted about it, and that first pass is where the measurement comes from. You can
always compress a `LOW YIELD` game on demand: highlight it in the TUI and press `c`.

**Deleting is judged differently.** Removing unused data (`--slim`, `--prune-assets`)
writes almost nothing, so it is worth doing even for a low percentage of a huge game.
It is gated by a small absolute floor instead, `min_prune_mib` (default 1): a 1 GB
saving on a 100 GB game is a prune, not a rewrite, and it is taken.

### Empty installs are ignored

Steam leaves behind directory stubs for downloads that failed, were interrupted, or were
never started — a folder with no files, or a 53-byte depot marker. Compressing those is
pointless noise, and they otherwise inflate your game count, so any directory holding
less than **1 MiB** of file data is skipped.

The threshold is a plain config file:

```sh
# MiB; 0 disables the filter entirely
echo 5 > ~/.config/btrfs-game-compressor/min_size_mib
```

`--status` always reports what was skipped and why, so nothing disappears silently:

```
libraries: 1   games: 225   pending: 12   compressed: 213
skipped:   10 empty install(s) under 1 MiB (Chorus, SteamVR, ENDLESS Legend 2, ...)
```

The size check and the modification check share a single `find` pass per game, so
skipping costs essentially nothing — a full scan of a 356 GB / 247k-file library takes
about half a second.

### Compression level

Defragmenting at a level that differs from the mount produces a library with mixed
compression levels and an unclear ratio. By default (`auto`) the level is read from
whatever the library's mount point is configured with, so a library stays uniform:

| Mount option | Level used |
|---|---|
| `compress-force=zstd:6` (SteamOS default) | 6 |
| `compress-force=zstd` (no level) | 3 (btrfs default) |
| `compress=zstd:1` | 1 |
| no `compress` option | 3, and you get a warning telling you to fix the mount |

`compress-force` takes precedence over `compress` when both are present. To pin a level
instead of mirroring the mount:

```sh
echo 6 > ~/.config/btrfs-game-compressor/compress_level   # 1-15, or 'auto'
```

**This is a real trade-off, not a formality.** Measured on a 222-game library, a 3 GiB
sample compresses to 49.1% at level 1 versus 46.2% at level 3 — so mirroring a `zstd:1`
mount costs about **6% more disk**. The gain from a higher level is very uneven:
data-heavy games like Factorio gain ~11%, while asset-heavy ones like Hades gain almost
nothing at any level, because their content is already compressed. Mirroring level 1
keeps write and recompression costs lower; if you are chasing space, try a higher level.

Removable media (a Deck's Btrfs SD card) is detected and reported, since its
storage and CPU trade-offs may differ from an internal drive.

## The game list and ratio table

**The full list of 225 measured games is below** (and in
[`GAMES.md`](GAMES.md)). The raw data is [`ratios/games.json`](ratios/games.json), and
the [best of it is shown above](#best-measured-games).

<details>
<summary><b>All 225 measured games</b> (saving at zstd level 1) — click to expand</summary>

<!-- GAME-LIST-START -->
| Game | Level | Saving | Samples |
|---|---:|---:|---:|
| 9 Kings | zstd1 | 41.4% | 2 |
| A Short Hike | zstd1 | 26.5% | 2 |
| ARSONATE | zstd1 | 44.8% | 2 |
| Alabaster Dawn | zstd1 | 55.8% | 2 |
| Along the Edge | zstd1 | 16.7% | 2 |
| Ape Out | zstd1 | 23.7% | 2 |
| Arco | zstd1 | 0.6% | 2 |
| Astro Prospector | zstd1 | 62.1% | 2 |
| Away | zstd1 | 60.9% | 2 |
| BALLxPIT | zstd1 | 42.0% | 2 |
| Baba Is You | zstd1 | 16.6% | 2 |
| BackToBed | zstd1 | 42.6% | 2 |
| Backpack Hero | zstd1 | 61.6% | 2 |
| Balatro | zstd1 | 7.9% | 2 |
| BerryBerryBerry | zstd1 | 35.3% | 2 |
| Bitburner | zstd1 | 62.6% | 2 |
| Blanc | zstd1 | 14.8% | 2 |
| Blasphemous | zstd1 | 11.3% | 2 |
| Bread & Fred | zstd1 | 54.6% | 2 |
| Brotato | zstd1 | 22.0% | 2 |
| Calm Down, Stalin | zstd1 | 38.1% | 2 |
| Carrion | zstd1 | 18.6% | 2 |
| Caveblazers | zstd1 | 33.3% | 2 |
| Caves of Qud | zstd1 | 26.2% | 2 |
| Celeste | zstd1 | 21.9% | 2 |
| Chained Echoes | zstd1 | 17.7% | 2 |
| Chants of Sennaar | zstd1 | 11.0% | 2 |
| Chef Knight | zstd1 | 61.8% | 2 |
| Click the Button | zstd1 | 39.6% | 2 |
| Closer the Distance | zstd1 | 53.8% | 2 |
| CloverPit | zstd1 | 62.3% | 2 |
| Cocoon | zstd1 | 34.8% | 2 |
| CodeTerraform | zstd1 | 21.1% | 2 |
| Coffee Talk | zstd1 | 51.2% | 2 |
| Confidential Killings | zstd1 | 15.4% | 2 |
| Control | zstd1 | 50.8% | 2 |
| Craftomation101 | zstd1 | 4.2% | 2 |
| Crop Rotation | zstd1 | 6.1% | 2 |
| Crown Siege | zstd1 | 49.6% | 2 |
| Crownhold | zstd1 | 29.4% | 2 |
| Cryptark | zstd1 | 22.6% | 2 |
| Cult of the Lamb | zstd1 | 14.7% | 2 |
| CultOfPiN | zstd1 | 19.9% | 2 |
| DELTARUNE | zstd1 | 16.2% | 2 |
| DREDGE | zstd1 | 12.0% | 2 |
| Database Detective | zstd1 | 83.6% | 2 |
| Dave the Diver | zstd1 | 15.0% | 2 |
| Dead Cells | zstd1 | 5.2% | 2 |
| Dead Estate | zstd1 | 5.6% | 2 |
| Death's Door | zstd1 | 47.2% | 2 |
| DemonLordJustABlock | zstd1 | 78.6% | 2 |
| Desktop Explorer | zstd1 | 50.9% | 2 |
| Disco Elysium | zstd1 | 6.3% | 2 |
| Dispatch | zstd1 | 0.0% | 2 |
| Divide by sheep | zstd1 | 10.2% | 2 |
| Dokimon | zstd1 | 10.4% | 2 |
| Dome Keeper | zstd1 | 18.7% | 2 |
| Don't Let It Starve | zstd1 | 30.0% | 2 |
| Donut County | zstd1 | 36.1% | 2 |
| Dorfromantik | zstd1 | 29.9% | 2 |
| Down in Bermuda | zstd1 | 12.1% | 2 |
| Downwell | zstd1 | 15.9% | 2 |
| Dungeons & Degenerate Gamblers | zstd1 | 6.0% | 2 |
| Duskers | zstd1 | 50.1% | 2 |
| Dwarf Eat Mountain | zstd1 | 22.4% | 2 |
| Dwarf Fortress | zstd1 | 0.0% | 2 |
| Emily is Away 3 | zstd1 | 53.2% | 2 |
| Enter the Gungeon | zstd1 | 16.3% | 2 |
| Exit the Gungeon | zstd1 | 27.8% | 2 |
| FAITH | zstd1 | 3.3% | 2 |
| FEED IT | zstd1 | 53.8% | 2 |
| FEED THE QUEEN | zstd1 | 20.7% | 2 |
| Factorio | zstd1 | 23.7% | 2 |
| Fallout Shelter | zstd1 | 14.8% | 2 |
| Felvidek | zstd1 | 17.2% | 2 |
| Forage Wizard | zstd1 | 82.7% | 2 |
| Forager | zstd1 | 11.2% | 2 |
| Fortune Mill | zstd1 | 39.6% | 2 |
| GRIS | zstd1 | 64.1% | 2 |
| Gamblers Table | zstd1 | 31.5% | 2 |
| Gambonanza | zstd1 | 49.7% | 2 |
| GeckoGods | zstd1 | 49.1% | 2 |
| Gladiabots | zstd1 | 57.9% | 2 |
| Gnorp | zstd1 | 11.3% | 2 |
| GoNNER | zstd1 | 57.5% | 2 |
| Gorogoa | zstd1 | 9.3% | 2 |
| Graveyard Keeper | zstd1 | 63.4% | 2 |
| Gravity Circuit | zstd1 | 0.0% | 2 |
| Great God Grove | zstd1 | 34.3% | 2 |
| Greedy_Greedy_Gnomes | zstd1 | 34.2% | 2 |
| Grey Hack | zstd1 | 73.4% | 2 |
| Grimm | zstd1 | 36.2% | 2 |
| Hacknet | zstd1 | 27.6% | 2 |
| Hades | zstd1 | 9.1% | 2 |
| Hades II | zstd1 | 0.0% | 2 |
| Happy Wheels | zstd1 | 41.8% | 2 |
| Hats and Hand Grenades | zstd1 | 7.5% | 2 |
| He is coming | zstd1 | 57.2% | 2 |
| Hellslave | zstd1 | 2.5% | 2 |
| HenryStickmin | zstd1 | 3.5% | 2 |
| Hindsight | zstd1 | 47.7% | 2 |
| Hollow Knight | zstd1 | 75.0% | 2 |
| Horripilant | zstd1 | 15.3% | 2 |
| Hotline Miami 2 | zstd1 | 10.8% | 2 |
| Hue | zstd1 | 61.2% | 2 |
| Hylics | zstd1 | 10.5% | 2 |
| HyperLightDrifter | zstd1 | 11.3% | 2 |
| ITTA | zstd1 | 25.0% | 2 |
| IdolsOfAsh | zstd1 | 28.5% | 2 |
| Intravenous | zstd1 | 0.4% | 2 |
| Iron Lung | zstd1 | 21.4% | 2 |
| Is This Seat Taken | zstd1 | 63.3% | 2 |
| It Has My Face | zstd1 | 42.8% | 2 |
| Just Ignore Them | zstd1 | 51.0% | 2 |
| KIDS | zstd1 | 71.2% | 2 |
| Kingdom | zstd1 | 17.0% | 2 |
| LISA | zstd1 | 3.6% | 2 |
| LISA The First | zstd1 | 7.8% | 2 |
| LUCKROT | zstd1 | 49.4% | 2 |
| Lil Gator Game | zstd1 | 12.6% | 2 |
| Limbo | zstd1 | 7.3% | 2 |
| Looking for Fael | zstd1 | 10.7% | 2 |
| Lootbound | zstd1 | 24.0% | 2 |
| Lossless Scaling | zstd1 | 77.8% | 2 |
| Lost Wiki Kozlovka | zstd1 | 30.9% | 2 |
| Luftrausers | zstd1 | 31.0% | 2 |
| M.O.L.E | zstd1 | 6.3% | 2 |
| MOLDRISE | zstd1 | 22.1% | 2 |
| MechHavoc | zstd1 | 90.0% | 2 |
| Megabonk | zstd1 | 57.3% | 2 |
| Melvor Idle | zstd1 | 57.9% | 2 |
| Mina the Hollower | zstd1 | 9.5% | 2 |
| Mind Scanners | zstd1 | 64.3% | 2 |
| Mindustry | zstd1 | 14.9% | 2 |
| Minit | zstd1 | 11.8% | 2 |
| Minutescape | zstd1 | 46.6% | 2 |
| Moldwasher | zstd1 | 59.7% | 2 |
| MonsterHunterRise | zstd1 | 0.0% | 2 |
| Monument Valley | zstd1 | 15.2% | 2 |
| Moonlighter | zstd1 | 50.0% | 2 |
| Moonrise Fall | zstd1 | 16.2% | 2 |
| Moonstone Island | zstd1 | 37.1% | 2 |
| Moventure | zstd1 | 63.3% | 2 |
| Mutazione | zstd1 | 12.3% | 2 |
| Necesse | zstd1 | 20.1% | 2 |
| Neon Abyss | zstd1 | 48.1% | 2 |
| Nerd Survivors | zstd1 | 69.8% | 2 |
| Neva | zstd1 | 63.3% | 2 |
| No, I'm not a Human | zstd1 | 56.6% | 2 |
| Nocturnal | zstd1 | 9.1% | 2 |
| Noita | zstd1 | 6.7% | 2 |
| ObraDinn | zstd1 | 52.9% | 2 |
| Oceaneers | zstd1 | 15.9% | 2 |
| Old Man's Journey | zstd1 | 11.0% | 2 |
| Ooo | zstd1 | 12.7% | 2 |
| OpenFront | zstd1 | 61.8% | 2 |
| Orb of Creation | zstd1 | 28.5% | 2 |
| Ori DE | zstd1 | 51.0% | 2 |
| Outer Wilds | zstd1 | 47.5% | 2 |
| P0 | zstd1 | 75.4% | 2 |
| PapersPlease | zstd1 | 47.6% | 2 |
| Paradox Soul | zstd1 | 21.8% | 2 |
| Peglin | zstd1 | 42.1% | 2 |
| Pikuniku | zstd1 | 36.9% | 2 |
| Pilgrims | zstd1 | 39.6% | 2 |
| Pizza Hero | zstd1 | 48.7% | 2 |
| Pizza Tower | zstd1 | 25.0% | 2 |
| Poco | zstd1 | 64.9% | 2 |
| Pony Island | zstd1 | 60.7% | 2 |
| Press Any Button | zstd1 | 6.8% | 2 |
| Prison Architect | zstd1 | 4.2% | 2 |
| REPLACED | zstd1 | 24.3% | 2 |
| Rain World | zstd1 | 26.9% | 2 |
| Reventure | zstd1 | 62.6% | 2 |
| Rune Dice | zstd1 | 67.3% | 2 |
| Sable | zstd1 | 9.5% | 2 |
| Sandustry | zstd1 | 29.1% | 2 |
| Scavland | zstd1 | 5.4% | 2 |
| Sea of Stars | zstd1 | 13.2% | 2 |
| Slay the Spire 2 | zstd1 | 39.5% | 2 |
| Slime Rancher 2 | zstd1 | 12.2% | 2 |
| Slots & Diapers | zstd1 | 27.2% | 2 |
| Sludge Life | zstd1 | 42.0% | 2 |
| Sol Cesto | zstd1 | 17.1% | 2 |
| Stacklands | zstd1 | 53.2% | 2 |
| Stardew Valley | zstd1 | 24.4% | 2 |
| Storyteller | zstd1 | 9.4% | 2 |
| Super Meat Boy | zstd1 | 22.5% | 2 |
| TOEM | zstd1 | 40.2% | 2 |
| TPH | zstd1 | 47.1% | 2 |
| Terraria | zstd1 | 17.4% | 2 |
| Thank Goodness You're Here! | zstd1 | 15.8% | 2 |
| The Binding of Isaac Rebirth | zstd1 | 2.6% | 2 |
| The Children of Clay | zstd1 | 43.8% | 2 |
| The Escapists | zstd1 | 18.7% | 2 |
| The Farmer Was Replaced | zstd1 | 48.2% | 2 |
| The Gardens Between | zstd1 | 38.5% | 2 |
| The Loopler | zstd1 | 48.5% | 2 |
| The Message from Deep Space | zstd1 | 45.8% | 2 |
| The White Door | zstd1 | 7.1% | 2 |
| The Wolf Among Us | zstd1 | 13.6% | 2 |
| Thronefall | zstd1 | 67.7% | 2 |
| Tiny Terry's Turbo Trip | zstd1 | 56.1% | 2 |
| Titan Souls | zstd1 | 2.5% | 2 |
| To the Moon Beachsode | zstd1 | 3.8% | 2 |
| Ultrapool | zstd1 | 64.3% | 2 |
| Unpacking | zstd1 | 63.4% | 2 |
| Untitled Goose Game | zstd1 | 49.2% | 2 |
| Valheim | zstd1 | 5.1% | 2 |
| Valiant Hearts | zstd1 | 3.6% | 2 |
| WHAT THE GOLF | zstd1 | 22.9% | 2 |
| Wall World | zstd1 | 83.0% | 2 |
| Webbed | zstd1 | 37.8% | 2 |
| Wizard with a Gun | zstd1 | 10.2% | 2 |
| WormsWMD | zstd1 | 64.2% | 2 |
| You Have to Win the Game | zstd1 | 46.2% | 2 |
| Zero Stress King | zstd1 | 25.0% | 2 |
| border pioneer | zstd1 | 25.2% | 2 |
| devildaggers | zstd1 | 10.7% | 2 |
| hotline_miami | zstd1 | 63.4% | 2 |
| lucid-blocks | zstd1 | 38.8% | 2 |
| missed messages | zstd1 | 38.7% | 2 |
| theendisnigh | zstd1 | 7.8% | 2 |
| worldbox | zstd1 | 39.8% | 2 |
| wtl | zstd1 | 68.3% | 2 |
<!-- GAME-LIST-END -->

</details>

It is **only a hint for games this machine has not compressed yet** — your own
`--benchmark` measurement always wins, and the table never triggers a write. It is
installed with the tool, so `--ratios` works offline on an installed system.

Look a game up before spending the disk I/O:

```sh
btrfs-game-compressor --ratios               # the whole table
btrfs-game-compressor --ratios "Factorio"    # one game
btrfs-game-compressor --ratios --json        # machine-readable
```

**The zstd level is part of the key.** A game's ratio is a property of *(game, level)*,
not the game alone, so every row names the level it was measured at and the tool only
uses a row that matches your library's mount. If your mount is `compress=zstd:1`, you
are shown the level-1 measurement, never the level-6 one. `--status` and `--notify`
surface the hint next to an unmeasured pending game, labelled
`table says ~72.5% (zstd1)` so it is never mistaken for a local number. A game the table
expects to save under `min_gain_pct` is flagged `likely low yield` in `--status` and
`--dry-run` — an advisory, never a block, because only a local pass can measure it for
real.

### Add your games

Contribute what you measured, so the next person benefits. From a clone:

```sh
btrfs-game-compressor --export-ratios > my-ratios.json   # after a compression pass
make ratios-merge FILE=my-ratios.json                    # fold it into the table
python3 ratios/merge.py ratios/games.json my-ratios.json --dry-run   # ...or preview first
```

`make ratios-merge` averages your samples into any existing row (weighted by sample
count), widens `min`/`max`, adds games the table has not seen, and regenerates
`GAMES.md` and the list above. Then commit and open a pull request.

**No checkout? One command offers it for you** (needs the GitHub CLI, logged in):

```sh
btrfs-game-compressor --submit-ratios
```

It shows exactly what will be sent, asks once, and opens an issue with your
measurements. The payload is game names, zstd level, saving and sample count — no
paths, no host details. A maintainer merges it. There is also a
[ratio submission issue form](https://github.com/pgm1207/btrfs-game-compressor/issues/new?template=ratio_submission.yml)
for pasting the output by hand.

Nothing is uploaded automatically: the data moves only through a pull request or issue
you can read. See [`ratios/README.md`](ratios/README.md) for the format and rules.

## Files

| Path | |
|---|---|
| `~/.config/btrfs-game-compressor/custom_libraries.txt` | extra library roots, one per line |
| `~/.config/btrfs-game-compressor/throttle.conf` | `0` = full speed, `1` = `nice`/`ionice` eco mode |
| `~/.config/btrfs-game-compressor/min_size_mib` | skip installs under this many MiB; `0` disables |
| `~/.config/btrfs-game-compressor/min_gain_pct` | hold back **rewrites** (compression) under this `%` last time; `0` disables |
| `~/.config/btrfs-game-compressor/min_prune_mib` | minimum saving for a **delete-only** prune; `0` disables |
| `~/.config/btrfs-game-compressor/keep_languages` | languages `--slim` keeps (codes or names) |
| `~/.config/btrfs-game-compressor/compress_level` | zstd level `1`-`15`, or `auto` to mirror the mount |
| `~/.config/btrfs-game-compressor/visual_target` | beta asset profile: native, 360p, 720p, 1080p, 1440p or 4K |
| `~/.config/btrfs-game-compressor/ratios_hint.json` | imported community table, used as a fallback hint only |
| `~/.config/btrfs-game-compressor/auto_update` | `1` (default) checks for a newer release once a day; `0` disables |
| `~/.local/state/btrfs-game-compressor/compressed_games.db` | what was compressed, when, and how much it saved |
| `~/.local/state/btrfs-game-compressor/compressed_games.db.bak` | the previous state file, kept before each rewrite |
| `~/.local/state/btrfs-game-compressor/dedupe_physical_history.db` | append-only before/after dedupe measurements, separate from ZSTD savings |
| `~/.local/state/btrfs-game-compressor/dedupe_completed.db` | successful dedupe timestamps used to decide whether a game is compacted |
| `~/.local/state/btrfs-game-compressor/dedupe/*.db.lock` | per-library locks serializing deduplication |
| `~/.local/state/btrfs-game-compressor/last_update_check` | timestamp of the last automatic update check |

Installed with the one-line installer, the tool, manpage and ratio table live under
`~/.local/bin` and `~/.local/share`; `--uninstall` removes them and leaves the two
directories above untouched.

Compression history is keyed by the full install path, so separate copies of the same
game in different Steam libraries keep independent status and savings data. Path fields
are percent-escaped, so separators and line breaks in valid Linux paths cannot corrupt
the database.

The file begins with a version header (`# btrfs-game-compressor state v2`). It is what
lets a future release recognise and migrate older records. History written by versions
that keyed only by game name is matched to its install path on first run, so upgrading
does not cost you the work already done; a name shared by more than one install is left
alone rather than guessed at. Before any rewrite the previous file is copied to `.bak`,
so a bad write can never lose your history. If something ever does go wrong,
`cp compressed_games.db.bak compressed_games.db` restores it.

`--notify` requires the optional `notify-send` command from libnotify. It reports only
`COMPRESSED`, `UPDATED` and `UNCOMPRESSED` games that need work; it never starts compression.

## Eco mode

Defragmenting a large library is I/O heavy. Settings → *Toggle Speed / Throttle mode*
wraps the command in `nice -n 19 ionice -c 2 -n 7` so it yields to anything else you are
doing. Use it if you game or build on the same disk while a batch runs.

## Troubleshooting

**It warns "has no compress= mount option".** Your filesystem is not mounted with
compression, so new extents would inherit none. Add `compress=zstd` to the mount in
`/etc/fstab` and remount, or `sudo mount -o remount,compress=zstd /mnt/games`. The tool
still defragments, but the result would not stay compressed without it.

**Savings say `n/a`.** The native backend could not read privileged extent metadata.
Authenticate through the interactive prompt, or inspect `sudo bgc-native measure
/path/to/game`. Compression history is retained even when measurement is unavailable.

**`--status` lists games I do not have.** Those are empty download stubs; they are
filtered out by `min_size_mib` (default 1 MiB). If you want to see them, lower it.

**A game is `LOW YIELD` but I want it compressed.** Lower or zero `min_gain_pct` in
[Files](#files), or highlight the game in the TUI and press `c`. Note this rewrites
the whole game, which costs SSD endurance, so it is gated on the ratio rather than an
absolute size.

**It did nothing on my filesystem.** Check the two `findmnt` commands in [Is this for
me?](#is-this-for-me). On `ext4`, `xfs` or `ntfs`, Btrfs compression does not apply.

**How do I update / uninstall?** `btrfs-game-compressor --self-update` and
`btrfs-game-compressor --uninstall`. Both are described in [Install, update,
uninstall](#install-update-uninstall).

**Is it safe to press `Ctrl+C` mid-run?** Yes. The interrupted game stays marked
pending and is retried next time; a game is only recorded once the `btrfs` call
succeeds.

**A texture looks wrong after the asset stage.** The texture stage is lossy and is
not verified in-game. If you applied with `--apply-assets` (backups kept), restore
with `btrfs-game-compressor --restore-assets "GAME"`. With `--compact-all-no-backup`
or the native `apply-no-backup` there is no restore copy: use Steam's "Verify
integrity of game files" to fetch the originals.

**Which games can use texture compression?** Run the read-only planner,
`btrfs-game-compressor --assets "GAME"`. It reports candidate logical bytes;
small sprites, thin textures and unsupported packed formats (Unity, Unreal) are
left untouched. The biggest supported wins in a typical library are Godot 4
`.ctex`; see [Texture compression](#texture-compression).

## Caveats

- **Compression is applied per extent at write time.** `btrfs filesystem defragment
  -czstd` rewrites existing extents so they become ZSTD-compressed. Files already stored
  with a compression algorithm are skipped.
- **A remount is needed for the flag to stick.** New writes only get compressed if the
  mount has `compress=zstd`.
- **It cannot make a game smaller than its compressed assets allow.** Already-optimal
  archives will barely move.
- **Experimental texture downscaling is lossy and unverified in-game.** It only
  replaces a texture when the rebuilt file is strictly smaller, preserves the
  source codec and rebuilds mips, and refuses small/thin textures, cubemaps,
  arrays, volumes, BC6H and float formats. A game may still rely on exact texture
  dimensions for UI or data. Unity and Unreal packed textures are untouched.
- **Expect a full disk read+write per game.** On a mechanical drive this is slow; on
  NVMe it is minutes for a whole library. Interrupting with `Ctrl+C` is safe — the game
  stays marked pending and is retried next time.
- **Snapshots share extents.** Native compressed-size measurements report referenced extent storage, which is
  what you actually get, but a snapshot taken before compressing will keep the old,
  uncompressed extents alive.
- **The self-update checksum is fetched from the same release.** It protects against a
  corrupt or truncated download, not against a compromised GitHub account. Signed
  releases are not implemented yet.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports and ideas welcome via issues. Run
`make check` before opening a pull request; it is what CI runs.

## License

[MIT](LICENSE)
