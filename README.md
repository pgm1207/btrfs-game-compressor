# btrfs-game-compressor

**Reclaim 15–40% of your game library on Btrfs — losslessly, with no change in game
quality and essentially no performance cost. Unusually compressible titles reach 90%
(`MechHavoc`), measured with `compsize`.**

[![CI](https://github.com/pablogonz12/btrfs-game-compressor/actions/workflows/ci.yml/badge.svg)](https://github.com/pablogonz12/btrfs-game-compressor/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-any%20Linux%20with%20Btrfs-informational)](#requirements)
[![Shell](https://img.shields.io/badge/shell-bash%204%2B-4EAA25)](#requirements)

If you game on Linux with a Btrfs filesystem — a Steam Deck, an Arch/CachyOS box, a
Fedora or Ubuntu desktop, a Bazzite handheld — there is a good chance a large part of
your library is sitting on disk **uncompressed**, even though compression is switched
on. This tool finds those games, compresses them with ZSTD, and remembers what it has
done so it never repeats work.

It is a single bash script. No daemon, no root service, no telemetry, no config files
outside your home. It tells you what it is going to do before it does it, never
touches a game that is running, and knows when re-compressing is not worth your time.

> **225 games already measured.** Browse the [full game list](GAMES.md) — best so far
> is `MechHavoc` at **90%** reclaimed, with [57 of 225 saving over
> 50%](#best-measured-games). Your own numbers come from `--benchmark`.

```console
$ btrfs-game-compressor --status
================================================================================
  BTRFS GAME COMPRESSOR v0.1.0   225 game(s) across 1 library(ies)
  [########################....] 92% compressed
================================================================================
GAME                                     STATUS               SIZE EXPECTED GAIN
--------------------------------------------------------------------------------
Hades                                    COMPRESSED          11.2G
Caves of Qud                             UPDATED             1.8G
Factorio                                 UNCOMPRESSED        2.1G  table says ~44% (zstd1)
--------------------------------------------------------------------------------
libraries: 1   games: 225   pending: 12   compressed: 213   low-yield: 0
```

## Table of contents

- [What it does](#what-it-does)
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

- **Finds** Steam libraries on Btrfs, from `libraryfolders.vdf` and Flatpak layouts,
  plus any roots you register.
- **Defragments** games with `btrfs filesystem defragment -r -czstd -L`, mirroring
  the zstd level your filesystem is already mounted with so a library stays uniform.
- **Remembers** what it did per install path, so a game is only re-examined when
  something actually changed, and never compressed twice for nothing.
- **Never touches a running game**, **stops on failure**, and is **safe to interrupt**
  at any time.
- **Tells you when it is not worth it** — a game that only saves a fraction of a
  percent is reported as `LOW YIELD` instead of costing you an hour.

One install for every distribution, and it keeps itself up to date:

```sh
curl -fsSL https://raw.githubusercontent.com/pablogonz12/btrfs-game-compressor/main/install.sh | sh
```

## Quick start

```sh
# 1. Install (any Linux; SteamOS included)
curl -fsSL https://raw.githubusercontent.com/pablogonz12/btrfs-game-compressor/main/install.sh | sh

# 2. Look, don't touch
btrfs-game-compressor --status

# 3. See exactly what a batch run would do
btrfs-game-compressor --dry-run

# 4. Do it, from the interactive UI
btrfs-game-compressor          # then press b for "batch"
```

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
  (textures, video, audio). Real-world savings are **15–40%** across a library, with
  outliers up to **90%** (`MechHavoc`) and duds near **0%** (`Arco`). Anything claiming
  a large saving on *every* game is not describing this tool.
- **It does not touch your saves, mods or Proton prefixes.** It only defragments files
  under `steamapps/common`. Steam's verification is unaffected, because the files are
  byte-for-byte identical.
- **It does not lose quality or data.** Compression is lossless: the game data is
  exactly the same, only stored more efficiently.
- **It does not speed games up.** There is no FPS gain to promise. The benefit is disk
  space, and often faster load times because the disk reads fewer bytes.
- **It does not run unattended.** No background daemon rewrites a library. The optional
  systemd timer only *notifies*.

### A word on performance

ZSTD decompresses very quickly, so on an SSD or NVMe the usual result is **no
perceptible FPS change**, and often slightly faster loads because fewer bytes are read.
The one place it can show is a sustained CPU-bound game on a low-power handheld, where
decompression competes for a 15 W budget. If you notice anything, it is that — and if
you do, `btrfs-game-compressor` never touches a game while it is running, so there is
nothing to undo mid-session.

## What makes it safe

The reason a tool like this is worth trusting is not the compression — that is one
`btrfs` call — it is the guard rails around it:

- **A running game is never defragmented.** The tool checks `/proc` for a process that
  has a file from the game's directory mapped, and re-checks immediately before every
  write. No more "Steam verified the game and my save died".
- **It never compresses anything you did not ask for.** No daemon, no background
  writes. The optional systemd timer only *notifies*.
- **It tells you when a game is not worth the time.** A game that only saves 0.3% of
  its size is reported as `LOW YIELD` instead of costing you an hour.
- **It stops on failure.** If the `btrfs` call fails, the game's saved state is left
  exactly as it was, so the next run retries it cleanly.
- **`--dry-run` truly changes nothing**, and `--status` is safe in scripts and CI.
- **Its own updates are checksum-verified.** See [Install, update,
  uninstall](#install-update-uninstall).

## Requirements

| | |
|---|---|
| **Required** | `bash` 4+, `btrfs-progs`, `awk`, coreutils/findutils/util-linux |
| **Optional** | `compsize` for savings figures; `notify-send` from libnotify for `--notify` |
| **Filesystem** | the library must be on **Btrfs**, mounted with a `compress=` or `compress-force=` option |

```sh
# Debian / Ubuntu  (compsize is packaged as btrfs-compsize)
sudo apt install btrfs-progs btrfs-compsize

# Arch / CachyOS
sudo pacman -S btrfs-progs compsize

# Fedora / Bazzite
sudo dnf install btrfs-progs compsize

# openSUSE
sudo zypper install btrfs-progs compsize
```

Without `compsize` a compressed game is still remembered as compressed, so it is not
re-done on every run, but no savings figure is shown and it is left out of `--stats`,
`--history` and `--benchmark`.

The script deliberately uses **only POSIX `awk`**, so it runs on a stock Debian/Ubuntu
where `/usr/bin/awk` is **mawk** rather than gawk. It is exercised in CI under both
`mawk` and `busybox awk`, so a gawk-only construct cannot creep back in unnoticed.
Beyond `awk` it relies on the standard GNU userland (`find -printf`, `stat -c`, `sed`,
`findmnt`) that every desktop Linux distribution ships by default.

Your filesystem must be mounted with compression enabled, otherwise new extents inherit
no compression and defragmenting is a no-op:

```sh
# persistent, in /etc/fstab
UUID=xxxx  /mnt/games  btrfs  compress=zstd,noatime  0  0

# or live
sudo mount -o remount,compress=zstd /mnt/games
```

`btrfs-game-compressor` checks this on startup and tells you if it is missing.

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
| **Ubuntu / Debian** | `compress=zstd` mount, apt-based dependencies |
| **openSUSE** | `compress=zstd` mount, zypper-based dependencies |

Derivatives (Pop!_OS, EndeavourOS, Nobara, etc.) are detected via `ID_LIKE` and inherit
their parent distro's behavior. Anything else says `platform: other` and still works.

On the immutable SteamOS/Steam Deck, use the one-line installer: it installs to
`~/.local/bin` on the persistent `/home` partition, so it survives OS updates — pacman
changes to the read-only `/usr` do not.

### Architecture

There is **no architecture-specific code**. The script is shell plus standard tools, so
it runs unchanged on `x86_64` (every Steam Deck and SteamOS device to date) and on
`aarch64` — the day SteamOS or the Steam client ships an ARM build that games on, this
tool is already valid there. Nothing needs porting, because there is nothing
architecture-dependent to port. The dependency is the *userland* (GNU awk, coreutils,
`btrfs-progs`), all of which are packaged for aarch64 on every major distribution.

This is stated as a fact about the code, not a tested configuration: no ARM handheld
running SteamOS exists to test on yet, so it is unverified rather than claimed.

## Install, update, uninstall

There is **one supported install**: the one-line installer, which works on every Linux
distribution and updates itself. Cloning the repo is only for contributing.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/pablogonz12/btrfs-game-compressor/main/install.sh | sh
```

It downloads the latest release archive, verifies it against the release's
`SHA256SUMS`, and installs the script to `~/.local/bin`, the manpage to
`~/.local/share/man/man1`, and the community ratio table to
`~/.local/share/btrfs-game-compressor/`. It **refuses to install** if the checksum does
not match. Because it is a single shell script with no build step, this works on every
distribution — including SteamOS, where `~/.local` is on the persistent `/home`
partition.

If you prefer to read a script before running it (a good habit), download it first:

```sh
curl -fsSL https://raw.githubusercontent.com/pablogonz12/btrfs-game-compressor/main/install.sh -o install.sh
less install.sh          # read it
sh install.sh
```

Options, either as environment variables or by editing the script:

| Variable | Default | Meaning |
|---|---|---|
| `PREFIX` | `$HOME/.local` | install under this prefix (`$PREFIX/bin/`) |
| `BTRFS_GAME_COMPRESSOR_VERSION` | latest release | pin a specific version, e.g. `0.1.0` |

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
| `Enter` or `c` | compress the highlighted game, or every checked game |
| `b` | batch-compress everything still pending |
| `f` | search / filter |
| `v` | show only pending games |
| `t` | savings statistics |
| `s` | settings (speed mode, library management) |
| `q` | quit |

### Non-interactive

Every mode below is safe to pipe, script or run from CI — none of them start the TUI.

```sh
# one-shot report
btrfs-game-compressor --status

# only what still needs compressing
btrfs-game-compressor --status --pending

# what a batch run would do, changing nothing
btrfs-game-compressor --dry-run

# register a library outside Steam's libraryfolders.vdf
btrfs-game-compressor --library /mnt/games

# machine-friendly
btrfs-game-compressor --status --no-color | column -t

# JSON for scripts
btrfs-game-compressor --status --json

# how much space has been reclaimed so far
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
| `-l, --library DIR` | add a library, then exit |
| `--pending` | restrict `--status` / `--dry-run` to pending games |
| `--no-color` | disable ANSI color (also honours `$NO_COLOR`) |
| `--json` | with `--status`, `--benchmark` or `--ratios`, emit machine-readable output |
| `--history` | print the recorded compression history and exit |
| `--stats` | print the total space reclaimed and exit |
| `--benchmark` | measure real per-game savings from recorded data |
| `--ratios [GAME]` | look up expected savings in the community table |
| `--export-ratios` | emit your measured ratios as JSON to contribute |
| `--import-ratios FILE` | use a ratios JSON as a fallback hint |
| `--render-ratios` | render the table as Markdown (maintainers) |
| `--check-update` | report whether a newer release exists, change nothing |
| `--self-update` | update this copy in place, verifying the release `SHA256SUMS` |
| `--uninstall` | remove the tool, manpage and systemd units; keeps history |
| `--yes` | with `--uninstall`, do not ask |
| `--notify` | send a read-only desktop notification for changed games |
| `--balance` | offer to rebalance each library's mount; reports and confirms, never unattended |

## How it decides what to compress

Each game gets one of five states:

- **COMPRESSED** — defragmented, and nothing in its directory has been modified since.
- **UPDATED** — previously compressed, but files changed afterwards (a patch, a save, a
  Proton update). Worth re-compressing.
- **UNCOMPRESSED** — never compressed by this tool.
- **LOW YIELD** — updated, but not expected to be worth the time. See below.
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

Both thresholds are plain config files, and setting either to `0` turns off that half of
the test:

```sh
# a game must have saved at least this much of its size last time
echo 5 > ~/.config/btrfs-game-compressor/min_gain_pct
# ...and at least this many MiB in absolute terms
echo 32 > ~/.config/btrfs-game-compressor/min_gain_mib
```

Raise them to be more selective, lower them to compress more eagerly. A game with no
history is never held back — it has to be compressed once before anything can be
predicted about it, and that first pass is where the measurement comes from. You can
always compress a `LOW YIELD` game on demand: highlight it in the TUI and press `c`.

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
nothing at any level, because their content is already compressed. If you chose level 1
to keep decompression cheap on battery, mirroring it is the right call; if you are
chasing space, pin a higher level.

Removable media (a Deck's btrfs SD card) is detected and reported, since that is where
both the space and the load-time benefit matter most.

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

Contribute what you measured, so the next person benefits:

```sh
btrfs-game-compressor --export-ratios > my-ratios.json   # after a pass
btrfs-game-compressor --import-ratios my-ratios.json     # preview it locally
```

`--export-ratios` groups your repeated measurements, resolves the level of each game's
mount, and emits a valid ratios document. Merge the entries into `ratios/games.json` in
a pull request; see [`ratios/README.md`](ratios/README.md) for the format and the rules.
Maintainers regenerate the browsable table with `make ratios-doc`. Nothing is uploaded
anywhere automatically: the data moves only through a pull request you can read.

## Files

| Path | |
|---|---|
| `~/.config/btrfs-game-compressor/custom_libraries.txt` | extra library roots, one per line |
| `~/.config/btrfs-game-compressor/throttle.conf` | `0` = full speed, `1` = `nice`/`ionice` eco mode |
| `~/.config/btrfs-game-compressor/min_size_mib` | skip installs under this many MiB; `0` disables |
| `~/.config/btrfs-game-compressor/min_gain_pct` | hold back games that saved under this `%` last time; `0` disables |
| `~/.config/btrfs-game-compressor/min_gain_mib` | hold back games saving under this many MiB; `0` disables |
| `~/.config/btrfs-game-compressor/compress_level` | zstd level `1`-`15`, or `auto` to mirror the mount |
| `~/.config/btrfs-game-compressor/ratios_hint.json` | imported community table, used as a fallback hint only |
| `~/.config/btrfs-game-compressor/auto_update` | `1` (default) checks for a newer release once a day; `0` disables |
| `~/.local/state/btrfs-game-compressor/compressed_games.db` | what was compressed, when, and how much it saved |
| `~/.local/state/btrfs-game-compressor/compressed_games.db.bak` | the previous state file, kept before each rewrite |
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
`UPDATED` and `UNCOMPRESSED` games and never starts compression.

## Eco mode

Defragmenting a large library is I/O heavy. Settings → *Toggle Speed / Throttle mode*
wraps the command in `nice -n 19 ionice -c 2 -n 7` so it yields to anything else you are
doing. Use it if you game or build on the same disk while a batch runs.

## Troubleshooting

**It warns "has no compress= mount option".** Your filesystem is not mounted with
compression, so new extents would inherit none. Add `compress=zstd` to the mount in
`/etc/fstab` and remount, or `sudo mount -o remount,compress=zstd /mnt/games`. The tool
still defragments, but the result would not stay compressed without it.

**Savings say `n/a`.** `compsize` is not installed. Install it with the package command
for your distro in [Requirements](#requirements); until then, games are still remembered
as compressed, they just have no figure attached.

**`--status` lists games I do not have.** Those are empty download stubs; they are
filtered out by `min_size_mib` (default 1 MiB). If you want to see them, lower it.

**A game is `LOW YIELD` but I want it compressed.** Raise or zero `min_gain_pct` /
`min_gain_mib` in [Files](#files), or highlight the game in the TUI and press `c`.

**It did nothing on my filesystem.** Check the two `findmnt` commands in [Is this for
me?](#is-this-for-me). On `ext4`, `xfs` or `ntfs`, Btrfs compression does not apply.

**How do I update / uninstall?** `btrfs-game-compressor --self-update` and
`btrfs-game-compressor --uninstall`. Both are described in [Install, update,
uninstall](#install-update-uninstall).

**Is it safe to press `Ctrl+C` mid-run?** Yes. The interrupted game stays marked
pending and is retried next time; a game is only recorded once the `btrfs` call
succeeds.

## Caveats

- **Compression is applied per extent at write time.** `btrfs filesystem defragment
  -czstd` rewrites existing extents so they become ZSTD-compressed. Files already stored
  with a compression algorithm are skipped.
- **A remount is needed for the flag to stick.** New writes only get compressed if the
  mount has `compress=zstd`.
- **It cannot make a game smaller than its compressed assets allow.** Already-optimal
  archives will barely move.
- **Expect a full disk read+write per game.** On a mechanical drive this is slow; on
  NVMe it is minutes for a whole library. Interrupting with `Ctrl+C` is safe — the game
  stays marked pending and is retried next time.
- **Snapshots share extents.** Savings measured by `compsize` are disk usage, which is
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
