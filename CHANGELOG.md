# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.0]: https://github.com/pablogonz12/btrfs-game-compressor/releases/tag/v0.1.0
