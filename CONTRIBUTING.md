# Contributing

Thanks for helping out. This is a single-file bash tool with no build step, so
the bar for a patch is low — but a few conventions keep it reviewable.

## Ground rules

- **Keep it one file.** The whole point is that `btrfs-game-compressor` is a
  single self-contained script you can drop on any Linux box. Do not introduce a
  dependency on a language runtime, a package manager, or a library we would have
  to vendor.
- **Standard POSIX-ish bash only.** Target bash 4+ (`${var,,}`, associative
  arrays). Avoid bash 5-only features so it still runs on Ubuntu 18.04-era bash.
- **No `set -e`.** The TUI deliberately tolerates non-zero exits from probing
  commands (`command -v`, `grep`, pipelines that may legitimately match nothing).
  Adding `set -e` will make the UI exit at random. Handle errors explicitly.
- **English only.** All user-facing strings, including comments.

## Before you open a PR

```sh
bash -n btrfs-game-compressor          # syntax
shellcheck btrfs-game-compressor       # lint
./test/smoke.sh                        # behavioural tests
make check                             # all three
```

`make check` is what CI runs. If it passes locally it will pass there.

## Things that will get a PR rejected

- Anything that compresses a game without asking, in a non-interactive mode.
  `--status` and `--dry-run` must never modify a game's data or the state file.
  Note that they do create the config and state *directories*, and the config
  files inside them, on first run — `--status` is not a pure read. A PR that
  needs a genuinely read-only mode should be explicit about that.
- Writing outside `~/.config/btrfs-game-compressor/` and
  `~/.local/state/btrfs-game-compressor/`. No files in `/etc`, no global state.
- Requiring `sudo` for anything other than `compsize` and `btrfs` itself.
- Locale-dependent number parsing. `LC_ALL=C` is exported on purpose.
- New interactive keys without a matching entry in the README key table.

## Reporting a bug

Open an issue with:

- output of `btrfs-game-compressor --version`
- your distro, and the `btrfs-progs` / `btrfs-compsize` versions
  (`pacman -Q btrfs-progs` or `dpkg -l btrfs-progs`)
- the mount options for the library's filesystem (`findmnt -no OPTIONS --target /path/to/library`)
- `btrfs-game-compressor --status --no-color` output

That last one usually identifies the problem immediately.

## Adding a feature

Non-interactive modes are the easiest place to add value, because they are
testable. If you add a flag, wire it through `parse_args`, add a `cmd_*`
function, document it in `usage()`, add a case to `test/smoke.sh`, and add a row
to the README options table.
