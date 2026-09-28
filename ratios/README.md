# Community compression ratios

`games.json` is a per-game table of measured Btrfs/ZSTD savings, contributed by
users. It is **only ever a hint**: for a game this machine has never compressed,
the tool can show what other people measured ("the table says ~72%"), but a local
`--benchmark` measurement always wins, and the table never decides whether
anything is compressed. It also never triggers a write — `--status` and `--notify`
are read-only.

Browsing the table without installing anything: [`../GAMES.md`](../GAMES.md) is a
Markdown rendering generated from this file.

## Why the zstd level is part of the key

A game's compression ratio is a property of **(game, zstd level)**, not of the
game alone. A file that compresses 49% at level 1 can reach 46% at level 3, and
data-heavy games like Factorio gain far more from a higher level than asset-heavy
ones like Hades, whose content is already compressed. Mixing levels in one number
would produce confident, wrong predictions for everybody.

So every row names the level it was measured at, the tool reads the level from the
library's own mount (`compress=zstd:N` / `compress-force=zstd:N`, default 3), and
a hint is shown **only when that level matches the row**. A `zstd1` measurement is
never applied to a `zstd6` library.

## Contribute a measurement

On the machine whose library you want to contribute, after at least one
compression pass:

```sh
# 1. Measure your own library first, so you are reporting real data.
btrfs-game-compressor --benchmark

# 2. Emit a ratios document for the games you have measured.
btrfs-game-compressor --export-ratios > my-ratios.json
```

`--export-ratios` reads the state database, resolves the zstd level of the mount
each game sits on, and emits a valid ratios document. Repeated measurements of the
same game at the same level are averaged into one row that also carries the number
of samples and the min/max, so a reader can weigh confidence.

You can dry-run it locally before touching the repository:

```sh
btrfs-game-compressor --import-ratios my-ratios.json
btrfs-game-compressor --ratios            # view the imported table
btrfs-game-compressor --ratios "Factorio" # one game
```

`--import-ratios` caches the file at
`~/.config/btrfs-game-compressor/ratios_hint.json` and uses it as a fallback hint
for games you have not measured. It refuses a file with no parsable entries, so a
bad merge can never be cached as if it were a table.

## Submit your numbers

### The fast path (one command)

From a clone of the repository:

```sh
btrfs-game-compressor --export-ratios > my-ratios.json
make ratios-merge FILE=my-ratios.json
```

`make ratios-merge` folds your file into `ratios/games.json` for you:

- a game the table has not seen is added,
- a game it already has is updated as a **sample-weighted average** of the two
  (`pct = (old_pct*old_samples + new_pct*new_samples) / (old_samples + new_samples)`),
- `samples` is summed and `min`/`max` are widened,
- `GAMES.md` and the list in the README are regenerated.

Then commit the changed files and open a pull request. Run `make ratios-merge
FILE=my-ratios.json DRY_RUN=1` first if you want to see the result without writing
it. (The merge tool needs `python3`; the tool itself does not.)

### No checkout? `--submit-ratios`

If you have the [GitHub CLI](https://cli.github.com) (`gh`) installed and logged in:

```sh
btrfs-game-compressor --submit-ratios
```

It shows exactly what will be sent, asks once, and opens an issue with your
measurements. The payload is the same `--export-ratios` document — game names, zstd
level, saving and sample count, nothing about you. A maintainer merges it.

### Or paste it by hand

The **"Submit compression ratios"** issue template does the same thing manually:
generate the file, then paste the whole `--export-ratios` output into the issue.

### Doing it by hand

Merge the entries from the `"games"` object of your file into the existing
`"games"` object in `ratios/games.json` (do not replace the file), then run
`make ratios-doc` and commit both.

No registration, account, or telemetry is involved: the data arrives as a normal
pull request or issue you can review line by line, and nothing is uploaded unless
you run the command yourself.

## Format

```json
{
  "schema": 1,
  "games": {
    "Factorio": {
      "zstd1": { "pct": 44.0, "samples": 2, "min": 43.5, "max": 44.5 }
    }
  }
}
```

| Field | Meaning |
|---|---|
| `schema` | Format version. Currently `1`. |
| `games` | Object keyed by the Steam game name (the directory name under `steamapps/common`). |
| `zstd1`, `zstd6`, … | One object per zstd level the game was measured at. Level `3` when the mount names no level, since that is btrfs's default. |
| `pct` | Saving as a percentage: `(uncompressed - on_disk) / uncompressed * 100`. The average when `samples > 1`. |
| `samples` | How many independent measurements the average is built from. |
| `min`, `max` | Smallest and largest `pct` observed, or equal to `pct` when there is one sample. |

Optional top-level fields such as `description`, `measured_with` and `updated` are
documentation only; the parser ignores anything outside `games`.

## What a measurement means

- **`pct` is a disk-space saving**, measured with
  `btrfs-compsize` after `btrfs filesystem defragment -czstd -L <level>`. It is
  not a guess and not an estimated ratio from file types.
- **The saving depends on the mount's level and on how uniform the library is.**
  Libraries defragmented before this tool mirrored the level may report a mixed,
  slightly pessimistic ratio; prefer fresh measurements.
- **Snapshots change the number.** A snapshot taken before compressing keeps the
  old, uncompressed extents alive, so the space `compsize` reports as free can be
  lower than the mechanical ratio suggests.
- **Small absolute games are still useful.** Because both the cost and the benefit
  of a pass scale with size, the ratio is what matters; a 200 MB game is as valid
  a data point as a 100 GB one.
- **Do not hand-edit `GAMES.md`.** It is generated. Edit `games.json` and run
  `make ratios-doc`.
