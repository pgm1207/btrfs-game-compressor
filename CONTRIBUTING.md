# Contributing

The application consists of a Bash interface and a small Rust backend.

## Ground rules

- **No external filesystem tools or runtime packages.** Use the bundled native
  backend for filesystem operations. Asset codecs are pinned and bundled at
  build time; the backend never launches external media tools. Release executables
  are statically linked. No proprietary encoders or runtime services.
- **Keep unsafe code limited to Linux UAPI calls.** Validate returned lengths,
  preserve file contents, and use kernel byte comparison before sharing data.
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
make native                            # build the static backend offline
make check                             # shell and Rust tests
BGC_TEST_BTRFS_DIR=/path/on/btrfs cargo test --manifest-path native/Cargo.toml
```

Builds require Rust/Cargo 1.89+ and a C linker. Real filesystem tests create and
remove only their own fixtures; omit BGC_TEST_BTRFS_DIR for ordinary CI tests.

## Things that will get a PR rejected

- Anything that compresses a game without asking, in a non-interactive mode.
  `--status` and `--dry-run` must never modify a game's data or the state file.
  Note that they do create the config and state *directories*, and the config
  files inside them, on first run — `--status` is not a pure read. A PR that
  needs a genuinely read-only mode should be explicit about that.
- Writing outside `~/.config/btrfs-game-compressor/` and
  `~/.local/state/btrfs-game-compressor/`. No files in `/etc`, no global state.
- Requiring `sudo` for anything other than native metadata measurement and balance.
- Locale-dependent number parsing. `LC_ALL=C` is exported on purpose.
- New interactive keys without a matching entry in the README key table.

## Reporting a bug

Open an issue with:

- output of `btrfs-game-compressor --version`
- your distro, kernel version (`uname -r`), and `bgc-native --version`
- the mount options for the library's filesystem (`findmnt -no OPTIONS --target /path/to/library`)
- `btrfs-game-compressor --status --no-color` output

That last one usually identifies the problem immediately.

## Adding a feature

Consult [ROADMAP.md](ROADMAP.md) for engine/format coverage, writer acceptance
gates and versioning. Push coherent tested increments; do not equate an audit with
texture/audio rewriting. Synchronize script, Cargo package/lock and manpage
versions at release boundaries. A `v*` tag publishes release archives through CI;
ordinary commits do not constitute a tagged release.

Unvalidated engine metadata reader drafts are separately tracked in
[docs/ENGINE_DEVELOPMENT_STATUS.md](docs/ENGINE_DEVELOPMENT_STATUS.md). Their
native-only routes require the `development-audits` Cargo feature, off by default;
enabling it is not support certification. A detached XNB exporter exists behind
that feature, but it has no installed apply route. Do not enable
it in normal packaging or auto-route its readers until validation passes. When
validation is authorized, exercise both feature states without replacing a live
or frozen compaction backend. If tests/builds are paused, keep drafts local and
do not push changes that would trigger CI.

Non-interactive modes are the easiest place to add value, because they are
testable. If you add a flag, wire it through `parse_args`, add a `cmd_*`
function, document it in `usage()`, add a case to `test/smoke.sh`, and add a row
to the README options table.
