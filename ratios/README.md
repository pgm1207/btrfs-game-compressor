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

## Open a pull request

1. Fork the repository and create a branch.
2. Paste the entries from the `"games"` object of your generated file into
   `ratios/games.json`, merging them into the existing `"games"` object (do not
   replace the file).
3. If the same game/level already exists, prefer merging your samples into the
   existing row over overwriting it: raise `samples`, and widen `min`/`max` if
   your measurement falls outside the current range.
4. Regenerate the browsable table and commit it alongside the JSON:

   ```sh
   make ratios-doc
   ```

5. Run the test suite (`make check`) and open the PR.

No registration, account, or telemetry is involved: the data arrives as a normal
pull request you can review line by line.

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
