# Scope

This document defines what version 0.1.0 is, so that the first release has a finish
line and everything after it is driven by real user feedback rather than by
speculation.

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

## Explicitly out of scope for 0.1.0

These are deliberate non-goals. They are listed so that they are decisions rather
than omissions, and so that proposals about them can start from a clear answer.

- **A rewrite in Rust or any other language.** See the reasoning below; the
  performance case does not exist.
- **Deduplication.** It is a different mechanism with a different toolchain
  (`duperemove`, `rmlint`). Proton `compatdata` deduplication is a legitimate and
  substantial project, but it is a separate project.
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

## Why not Rust

Measured, not assumed. On this repository's largest realistic test case — 225
games, 246k files, 356 GB:

| | |
|---|---|
| Total script overhead, `--status` | 1.6 s |
| Of which: `find` walking 246k files | ~0.5 s, and that is the kernel's work |
| Actual compression, per game | minutes, I/O bound |

The script is roughly **0.1% of the wall-clock time** of the operation it exists
to perform. The compression is done by `btrfs filesystem defragment`, a C program
this tool invokes. Rewriting the surrounding orchestration in Rust would make the
1.6 s report perhaps 0.4 s — a saving nobody will ever notice next to minutes of
defragmentation.

The performance actually available was taken with three deletions: not calling
`du` on trees we had already walked, not forking `basename` per game, and not
forking an `awk` per game to format a number. 2.63 s to 1.6 s, in Bash, with no
behaviour change.

### The real argument is distribution, not speed

The question was whether the code could move to Rust "without making the user do
anything crazy". It could not, and that is the decisive point:

- **Bash is `curl | sh`.** The artifact is a text file. It runs on any GNU-userland
  Linux, on any CPU, with no build step, no toolchain, no glibc version to match,
  and no binaries to sign. A user can read it before running it, which for a tool
  that calls `sudo btrfs` is worth a great deal.
- **A Rust binary is per-architecture.** Shipping it means building and publishing
  `x86_64` and `aarch64` artifacts, choosing static vs glibc, signing them, and
  keeping per-distro packages and the release workflow in step. A user on an
  unusual libc or an older distro gets a segfault or a version error instead of a
  clear bash failure. That is exactly the "crazy stuff" to avoid.
- **The TUI would need a framework dependency** (ratatui/crossterm), which is a
  larger and more volatile surface than the ~2000 lines of shell here.
- **Every distro already has the runtime.** There is no `bash` to install.

For a tool whose whole selling point is "drop one file and it works", the portable
shell script is not the compromise; it is the feature. The ARM question reinforces
this: because there is no architecture-specific code, the same file is already
valid on `aarch64` the day SteamOS ships there. A prebuilt Rust binary would need a
second artifact for that.

A rewrite becomes worth revisiting only if profiling ever shows the script itself,
rather than `btrfs`, dominating a real workload — and if a distribution story
exists that is as frictionless as a single signed script.

## How self-updating is done safely

This tool runs `sudo btrfs filesystem defragment`, so replacing its own file is a
privileged operation and has to be defended accordingly:

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
