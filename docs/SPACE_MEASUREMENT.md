# Reproducible storage-space measurement (experimental CLI helper)

`tools/space-delta.py` records two **read-only** measurements before and after
optimizing a game. It works even when the game is on ext4, because filesystem
available space is queried with `statvfs`. Native Btrfs extent accounting is
optional. This tool **does not optimize or rewrite game files**.

## Example

```sh
# Replace this path with the real installed game directory.
GAME="/mnt/games/steamapps/common/My Game"

# Keep reports OUTSIDE the measured filesystem when practical.
python3 tools/space-delta.py capture "$GAME" \
  --backend ./bgc-native --output /tmp/bgc-before.json

# Apply only an explicitly chosen, tested optimization to the game here.

python3 tools/space-delta.py capture "$GAME" \
  --backend ./bgc-native --output /tmp/bgc-after.json

python3 tools/space-delta.py compare \
  /tmp/bgc-before.json /tmp/bgc-after.json \
  --output /tmp/bgc-comparison.json
```

Running without `--backend` captures filesystem-wide space but emits null
extent fields. The script never invokes `sudo`. If the native backend cannot
read Btrfs extent metadata because of insufficient permission, wrong filesystem
or an unsupported kernel, the report contains `"status": "unavailable"` and
nulls, **not zero savings**.

## Fields and interpretation

- `filesystem_available_delta_bytes`: after minus before, the change in
  available filesystem-wide bytes. Positive means more capacity available in
  this observation. It is **not** proof the game optimization caused that gain.
- `filesystem_free_delta_bytes`: also includes bytes unavailable to ordinary
  users, depending on filesystem and allocation policy.
- `game_extent_disk_reduction_bytes`: before minus after, native Btrfs
  referenced game-data footprint. Requires valid native measurement in both
  snapshots.
- `game_raw_extent_reduction_bytes`: before minus after, pre-compression
  referenced extent bytes (not a complete user-file logical size).
- `game_referenced_extent_reduction_bytes`: before minus after, native
  referenced byte metric, which may include blocks still referenced elsewhere.

The tool emits signed integers, including negative results when space grows;
unmeasured metrics remain null. JSON reports carry a `schema_version` and
identify the game root and device. Comparing different game roots/devices is
rejected. Use a quiescent system for serious benchmarks; concurrent Steam
downloads, caches, snapshots, backups, quotas and delayed allocation invalidate
simple causal interpretations.

These metrics are not a substitute for an independent game launch/playback test
or visual and audio quality checks after lossy optimization. Restoration copies
may retain the original physical blocks until finalized.

## Compare Zstd compression levels without modifying games

`test/measure-compressed.py` can copy a representative **ordinary file**
into uniquely named temporary directories on a Btrfs scratch filesystem.
It applies the requested compression policy **only to each temporary directory**,
not to the original file, parent directory, game or Steam library.

```sh
python3 test/measure-compressed.py \
  --scratch-dir "/mnt/games" \
  --zstd-level 1 --zstd-level 3 --zstd-level 6 --zstd-level 9 \
  --repetitions 3 --json \
  "/path/to/a/representative/game-asset.bin" > /tmp/bgc-level-probes.json
```

The scratch directory must exist, be writable and reside on the filesystem
whose policy you want to test. Setting a Zstd policy requires `btrfs-progs`;
the program aborts if policy assignment fails. Without any `--zstd-level`,
the trial uses the scratch directory's inherited filesystem policy, which can
also be measured on non-Btrfs filesystems. Each result contains the observed
filesystem-wide USED byte delta from writing one copy, not an assertion of
exclusive compressed extents, game-wide savings or a throughput benchmark.
The copies are removed automatically even when a trial fails.

**Caution:** Each trial writes a complete disposable copy of the input file.
Use small representative assets first and ensure the scratch filesystem has
room for the copy. Avoid concurrent writes/downloads while sampling and do
not extrapolate small-file sample results to an entire game. A higher Zstd
level can increase CPU work, and the smallest sample result is not always
the best setting for game launch times or resource usage. Disable automatic
Steam game updates for affected games only if you understand Steam's update
semantics; Steam may replace modified files when it verifies or updates them.
