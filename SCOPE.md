# Scope

This document records the project's current scope and goals. The initial 0.1.0
finish line is retained below; the current extension adds safe, measurable extent
deduplication to the existing compression workflow.

The short version: **a terminal tool that finds Steam games wasting space on a
Btrfs filesystem, compresses them, and remembers what it has already done.**
Everything that is not that is either deferred or explicitly out of scope.

## Why the tool exists

Btrfs compresses on write, so a `compress=zstd` mount ought to shrink game
installs automatically. It does not, because **Steam preallocates game files with
`fallocate`**, creating unwritten extents that never get compressed. A fresh
Baldur's Gate 3 install sits 90% uncompressed until it is defragmented. See
[steam-for-linux#12974](https://github.com/valvesoftware/steam-for-linux/issues/12974).

This is worst on SteamOS and the Steam Deck, which mount `/home` and btrfs SD cards
with `compress-force=zstd` by default: the compression is genuinely enabled, and
the installs are genuinely still uncompressed. And it is not a one-time fix —
every Steam update rewrites files and undoes the compression on them again, so the
work has to be repeatable and stateful.

## In scope for 0.1.0

1. **Discovery** of Steam libraries from `libraryfolders.vdf` (native, `~/.steam`
   and Flatpak layouts) plus user-registered roots.
2. **State tracking** so a game is only re-examined when something actually
   changed: `COMPRESSED`, `UPDATED`, `UNCOMPRESSED`, `LOW YIELD`.
3. **Compression** via `btrfs filesystem defragment -r -czstd -L`, with the level
   mirroring the library's mount so the library stays uniform.
4. **Empty-install filtering**, because Steam leaves behind directory stubs from
   failed or interrupted downloads.
5. **Correct mount detection**, including `compress-force=`, which is what
   SteamOS actually uses.
6. **A TUI** for interactive selection, and **non-interactive modes** (`--status`,
   `--dry-run`) for scripting.
7. **Cost awareness**: an updated game whose previous saving was too small to be
   worth the time is reported as `LOW YIELD` and held back, rather than being
   re-compressed on every patch. Judged on the ratio, because cost and benefit both
   scale with size, so the size itself cancels out.
8. **Safety**: a dependency preflight, a warning when a library is not mounted
   with compression, and no writes to a game that is currently running.
9. **Packaging**: a single one-line `curl | sh` installer that works on every
   distribution, plus a manpage. One supported channel, not one per distro.
10. **Self-update**: `--self-update`, a read-only `--check-update`, and a
    throttled automatic check, so users on any distribution (SteamOS included) do
    not have to fetch a new release by hand. Every update verifies the release
    `SHA256SUMS` before replacing the script, and the tool refuses to update
    itself when a package manager owns the file, deferring to that package
      manager instead.
11. **Native filesystem backend (0.2.0)**: bundle a static, dependency-free Rust
    executable for compression, measurement, deduplication and optional balance.
    Keep Bash for the interface. No btrfs-progs, compsize or duperemove runtime
    dependency. Use bounded temporary indexes, per-library locks, sector-aligned
    requests, and kernel byte comparison. Preserve logical contents and stop on
    interruption. Build and package x86_64 and aarch64 executables separately.

## Current project goal: reduce real game-library disk usage

Extend compression with Btrfs extent deduplication and measure the combined
result against the library's actual allocated space. The motivating reference is
the reported Helldivers 2 reduction from roughly 140 GB to 20 GB after the game's
developers removed redundant packaged data. That is an inspiration and a
benchmark question, not an expected result from this tool: the native backend can share
identical extents, but it cannot remove near-duplicates or redesign a game's
asset bundles.

Filesystem compression and deduplication preserve Steam file contents.
The optional [BETA] asset stage follows compression and precedes final
deduplication. Profiles resize supported loose images, re-encode simple legacy
BC1/BC2/BC3 DDS textures, and reduce simple mono/stereo PCM or 32-bit-float WAV.
Non-native profiles produce integer PCM. Ultra
Performance additionally converts PCM to 8-bit using deterministic TPDF dither;
higher tiers target up to 16-bit without up-converting. Resized 8-bit RGB/RGBA images use tiered
color quantization with alpha retained. File paths and
containers stay intact; original bytes stay in a restore copy until the user
tests the game and explicitly discards it. Hades v7 LZ4 packages can be recompressed
losslessly without changing decoded chunks, textures or atlas manifests. A
Lossless profile disables image resizing and WAV resampling while enabling this
package pass. Other packed engine assets, Bink and encoded FMOD audio are
    inventoried but skipped by installed apply. WAV is not converted to Opus because game
loaders expect their declared codec/container. Hollow Knight and Hades store
many assets in containers; only Hades v7 LZ4 PKG recompression is implemented. This stage may
change visual/audio quality and does not claim universal reduction or faster
runtime.

An experimental, separate `--export-fmod QUALITY FILE OUTPUT` command can
decode/re-encode mono/stereo Vorbis and rebuild FSB5 v1 or single-FSB RIFF/FEV
banks using bundled codecs. It exports to a new file only, preserves declared
timing and non-codec metadata, and rejects unknown layouts. It is deliberately
not part of automatic asset replacement until in-game playback/seek tests pass.
Bink 2 re-encoding remains unimplemented.

Additional compiled-in, experimental export-only paths support lossless UnityFS
v6–8 LZ4 bundle recompression, standalone Godot PCK v1–3 duplicate-resource
sharing, lossy Godot 3 PCK v1 `.mp3str`→Ogg Vorbis audio rewriting, and lossy
Godot 4 PCK `.ctex` texture downscaling. The texture path decodes supported GST2
encodings (WebP/PNG and BC1/BC2/BC3/BC7) with bundled codecs, resizes to the
selected profile's longest edge, and re-encodes in the same format; unsupported
encodings are copied unchanged. The Godot 3 path decodes `AudioStreamMP3` with a
pure-Rust decoder, writes Ogg Vorbis under the same entry name, and retypes the
matching `.import` stub; other entries are copied unchanged. Both packs are
rebuilt with relocated offsets and recomputed MD5, then re-parsed, and untouched
entries are byte-compared. Finished output bytes are verified, existing
destinations refused, and logical savings/output-write efficiency checked before
writing. Neither path is part of installed auto-apply. Unity Texture2D/`.resS`
resizing, Amplify virtual textures, Godot 3 `AudioStreamSample`/`.stex`
rewriting, Godot 4 non-GST2 resources, Basis Universal/ETC/ASTC/half-float
textures, and Unreal/IoStore repacking remain unimplemented. A native Unreal Pak
footer audit reports encryption and declared codecs without claiming that it
understands actual entry contents or can safely switch codecs.
`--audit-container` exposes these format-specific investigations.
See `test/ENGINE_EXPERIMENTS.md` for reproducible negative results and current
resource bottlenecks; logical improvements alone are not physical savings.

A read-only `--variants GAME` report detects redundant asset suites by
directory shape alone (architecture/renderer builds, resolution tiers, platform
folders, language packs), and `--slim GAME` acts on the unambiguous ones in a
single offline, reversible pass (keep the highest resolution tier and the
host-platform build). A `keep_languages` setting lets the same pass drop every
language pack the user did not select (localized audio, subtitles, `locale/`
data); a group is only touched when a kept language is present, so a language
set is never emptied, and the user is warned to keep the startup language. Both
apply to arbitrary games because they parse no file formats and run no runtime
component.

Removing language data does not by itself rewrite an engine's built-in language
menu; whether an unwanted language disappears from the in-game list depends on
whether the engine enumerates the data. The goal is to remove unused bytes
safely and reversibly, not to patch engine code. Actual media reduction (capping mip chains or downscaling textures
to the played resolution, re-encoding video/audio) requires per-engine container
support and is therefore planned as plugins (Unity, Unreal, Godot, Hades PKG),
not as one universal rewriter.

Two opt-in commands remove content a released build does not load, moving it
into the same restorable backup tree rather than discarding it. `--prune-assets`
removes developer debug symbols (`*.pdb`, `*.ilk`). `--prune-fallbacks` also
removes low-resolution (720p) and BC3 texture/video fallback suites, and is only
safe when the machine renders the full-resolution assets. Neither re-encodes or
recompresses anything and neither changes quality on the assets actually used;
both require terminal confirmation and are reversible with `--restore-assets`.

## Explicitly out of scope for 0.1.0

These are deliberate non-goals. They are listed so that they are decisions rather
than omissions, and so that proposals about them can start from a clear answer.

- **A rewrite in Rust or any other language.** See the reasoning below; the
  performance case does not exist.
- **Proton `compatdata` deduplication.** Prefixes are mutable user data and need
  different discovery, safety and reporting rules. This project only deduplicates
  game content under Steam's `steamapps/common` tree.
- **A GUI or Decky Loader plugin.** A Decky plugin is the obvious way to reach
  Steam Deck users in the interface they already use, and it is deliberately not
  part of 0.1.0. It is a different artifact with a different runtime (a Python
  backend and a React frontend loaded into the Steam UI), a different release
  cadence, and a second way to call the privileged `btrfs` path at a moment when
  the user is mid-game. If it is built, the agreed shape is a **separate,
  future fork** that shells out to this CLI once the CLI is stable, rather than a
  second implementation folded into this repository. Keeping the contract here —
  stable `--status --json`, stable exit codes, no writes without confirmation —
  is what makes that fork cheap later. Until then, the terminal tool is the
  product, and any Decky work is out of scope.
- **Compressing anything that is not a Steam library.** Lutris, Heroic and
  non-Steam prefixes may well deserve the same treatment; they are not this tool.
- **Managing the mount options for you.** The tool reports a missing or wrong
  `compress=` and tells you what to run. It does not edit `/etc/fstab`, because
  silently changing how a filesystem is mounted is not a decision a game
  compressor should make.
- **Automatic compression of updates.** Detecting that a game updated is cheap and
  safe, and the tool does it on every run; *acting* on it unattended is a different
  proposition. A background daemon that takes root and rewrites a Steam library
  without being asked has to be right every time, and the failure modes are bad
  rather than cosmetic: a full disk of I/O on a Deck drains the battery, an
  ejectable SD card can disappear mid-write, and any false positive in change
  detection becomes an unwanted hour-long rewrite. Detection is worth having and
  costs nothing to re-run; the write is the part that should stay opt-in.
  What *is* in scope is the safe half: a **watch-only** systemd user timer (shipped
  in `systemd/`) that runs `--notify` twice a day, reports which games are worth
  revisiting, and never writes. It costs nothing to re-run and gets most of the
  value with none of the risk. Auto-compression remains a non-goal.
- **Per-game compression levels.** One level per filesystem, chosen by you.

## Candidate for 0.1.0, still to do

- [x] `btrfs balance` as an explicit, opt-in companion step. The canonical SteamOS
      guidance pairs defragment with `btrfs balance start -m`; we do only the
      first half. Now `--balance`: it reports every mount it would touch, prints
      the exact command, and refuses to run unattended or without a `y` at a
      terminal.
- [x] Never defragment a game that is currently running. A game with a live
      process holding a file under its install directory is reported as `RUNNING`
      and held back, and is re-checked immediately before the write rather than
      only at discovery, so a game launched during a long batch is not rewritten
      underneath itself.
- [x] Remove the accepted-but-unimplemented `--all-libraries` flag. A flag that
      silently does nothing is a bug.
- [x] Reconcile the changelog's claim about long game names with what the TUI
      actually does. Names are now clamped to the column width in the TUI,
      `--status` and `--dry-run`, on a UTF-8 character boundary.
- [x] Correct the contributing guide's claim that `--status` and `--dry-run`
      never touch the filesystem; they currently create config files on first run.
- [ ] Record a demo for the README.

## Completed after 0.1.0

- Per-install history keys and escaped state fields, so repeated game names and
  delimiter characters in paths do not share or corrupt records.
- Verified install archives, reproducible package tarballs, and a tag-triggered
  GitHub release workflow that creates the release page and checksum manifest.
- JSON status output and read-only desktop notices for changed games.
- VDF token parsing for quoted and escaped library paths.
- Regression coverage for compression failures and unusual library paths.
- `--history` and `--stats` for reading the recorded savings without opening the
  database by hand, plus a colour-coded `--status` with a progress bar.
- A watch-only systemd user timer in `systemd/`, installed with `make service`.

## After 0.1.0

Reasonable directions, in rough order of how much they would help people. None of
these are promises.

- Flatpak, for SteamOS reach. Needs a deliberate answer about the privileged
  `btrfs` call from inside a sandbox.
- Distribution packages, if the one-line installer ever stops being enough.
- Being mentioned where SteamOS Btrfs users already look.
- History-driven scheduling, now that state tracking has proven itself.
- Localisation.

## Bash interface, native filesystem backend

The 0.2.0 design keeps the interactive workflow and existing state in Bash. A
small Rust backend directly calls the Btrfs interfaces for compression, extent
measurement, deduplication and balance. No TUI framework or third-party Rust
crates are used. Static architecture-specific binaries ship with releases, so
users need neither Rust nor separately installed filesystem tools. The source
build requires Rust/Cargo and a linker; normal installs use prebuilt artifacts.

## How self-updating is done safely

The bundled backend may run with elevated privileges for measurement and balance,
so application updates must be validated:

- **The package manager wins.** If the script is owned by a package manager (a
  distro package under `/usr`, Homebrew, or Nix), self-update refuses and tells
  the user to update through it. That stops the two mechanisms fighting each
  other. Only a user-writable install (the one-line installer's `~/.local/bin`)
  updates itself.
- **Every update is checksum-verified** against the release's `SHA256SUMS`, the
  same way the installer already verifies a fresh install. A release whose
  archive does not match is rejected and the running file is left untouched.
- **It is explicit and throttled.** `--self-update` does it on demand,
  `--check-update` only reports, and the automatic check runs at most once a day
  so a normal run is not a network call. `auto_update=0` turns it off.
- **The update takes effect on the next run.** The running process is never
  re-executed mid-operation.

This trades a small amount of supply-chain surface (the download host plus the
release checksum) for the thing users actually asked for: never fetching a
release by hand again, on any distribution.
