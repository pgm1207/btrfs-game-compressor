# Scope

This document records the project's current scope and goals. The initial 0.1.0
finish line is retained below. Current release is 0.3.0, adding
native extent deduplication and opt-in format-aware asset optimization. See
[SUPPORT.md](SUPPORT.md) for tested coverage and [ROADMAP.md](ROADMAP.md) for the
engine support chart and delivery/versioning plan.

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

Filesystem compression and deduplication preserve Steam file contents. The
optional [BETA] asset stage follows compression and precedes final deduplication.
Native is byte-preserving and Lossless never degrades assets. Lossy profiles are
explicitly selected and unknown formats are skipped, without per-game recipes.

Implemented beta paths include supported loose images and DDS (legacy BC1/2/3 and
32-bit BGRA/RGBA, plus DX10 BC1/2/3/4/5/7 and R8G8B8A8/B8G8R8A8, decoded and
re-encoded with a rebuilt mip chain), simple mono/stereo PCM/float WAV, Hades v7
same-codec lossless LZ4 PKG, plain standalone Godot 3 PCK audio/GDST textures and
supported Godot 4 PCK GST2 textures, and an export-only texture downscaler
(`--texture-compress`, `--texture-compress-tree`).
Texture policy preserves small/thin and known-atlas assets, uses soft profile caps
with an original/logical half-size budget, and retains at least 7-bit explicit RGB
precision. Audio quality follows the asset profile, not the Zstd level. WAV is not
renamed or replaced with Opus. Godot 3 MP3 resources can change to Vorbis only with
their matching resource/import metadata updated.

Supported standalone FMOD FSB5 Vorbis and single-FSB RIFF/FEV banks now have
bounded main-pipeline apply as well as separate exports. Codebook, waveform and
savings gates preserve unknown/poor candidates. Declared timing, loop/event
metadata and identities are retained; encoded packet/seek offsets are rebuilt.
Main-pipeline files are limited to 256 MiB; this is beta and does not certify
in-game playback or seeking. Embedded Unity/Unreal audio remains unsupported.
Bink 2 re-encoding is not implemented.

Loose-file originals remain in a restore copy until the user tests and explicitly
finalizes them. Supported Godot PCK applies have no restore copy; recovery uses
Steam verification. Packs are rebuilt with relocated offsets/recomputed hashes,
then independently re-parsed and untouched resources verified. No games are
launched or killed for testing, and installed games are not modified as test data.

UnityFS v6–8 LZ4/HC recompression and Godot PCK v1–4 duplicate sharing remain
export-only. Unity Texture2D/`.resS` rewriting, Amplify virtual textures, Godot 4
audio/non-GST2 resources, Basis/ETC/ASTC/half-float textures, Unreal cooked texture
rewriting and Pak/IoStore repacking are not implemented. The Unreal Pak footer
audit reports encryption and declared codecs and checks bounded unencrypted
primary-index SHA1; legacy v1–9 additionally checks directory/data-header consistency
and bounded stored-payload hashes, and modern v10/v11 verifies path-hash and
directory-index SHA1 before classifying bounded encoded entries. Compressed/encrypted
payloads stay opaque. IoStore TOC v1–8 headers expose counts, security flags,
minimum extents and metadata-only regular companions, not chunk tables,
signatures, directory contents or `.ucas` data.
A bounded Unity SerializedFile v17–22 audit reports version/type-tree/class
evidence and supported type-tree Texture2D fields. Declared stream paths receive
metadata-only same-directory extent checks, not pixel reads or ownership proof.
A read-only `unityfs-inventory` decodes a UnityFS bundle in memory and summarizes
its SerializedFile nodes. Type trees were present in sampled Addressables bundles
and absent in sampled standalone player files; bundles can also omit trees.
This pass counts Texture2D objects but does not inspect their pixels or stream
ownership. No Unity writer is enabled; schema/field-span handling, reference and
storage resolution, and container rebuild (`.resS` relocation, object-table
shifts, CRC/catalog integrity) remain prerequisites. See
[the next compatibility design](docs/ENGINE_COMPATIBILITY_NEXT.md) for the
research and draft implementation sequence. Native-only unvalidated reader work
for these and additional formats is tracked separately in
[engine development status](docs/ENGINE_DEVELOPMENT_STATUS.md); no draft promotes
tested support or adds an installed writer.
`--audit-container` exposes the existing content-detected audits; detailed bundle
node inventory is currently native-only via `unityfs-inventory`. Audit, export
and apply are distinct capability levels in the roadmap.
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
