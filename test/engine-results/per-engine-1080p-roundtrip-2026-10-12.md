# Per-engine 1080p (Balanced) reversible apply — 2026-10-12

Scope: validate the current `main` (`2bec744`, 0.3.1) against the real Steam
libraries on the Btrfs mount `/mnt/storage/Games`, one game per detected engine,
using the **Balanced (1080p)** asset profile through the native backend the TUI
calls (`bgc-native assets apply balanced LEVEL DIR`). Every apply retained its
restore copy and was followed by `bgc-native assets restore native 0 DIR`.

Nothing here is runtime/gameplay validation, and no figure is net physical
savings. `apply` figures are **logical** transform sizes; `retained` is the
restore copy, not freed space.

## Environment

- Repo `/home/pgm1207/Documents/Scripts/btrfs-game-compressor`, `main` @ `2bec744`.
- Native `bgc-native` rebuilt (`make native`).
- Mount: `/dev/nvme1n1p1` on `/mnt/storage/Games`, `rw,noatime,compress=zstd:1`.
- No game process running during any test (only the Steam client).

## Test suites

| Suite | Result |
| --- | --- |
| `make check` (syntax + native unit + Python + smoke) | native `172 passed`, Python `34 OK`, smoke `522 passed / 0 failed` |
| `BGC_TEST_BTRFS_DIR=/mnt/storage/Games cargo test --test filesystem` | `11 passed; 0 failed` on the real Btrfs mount |

The real-mount run exercises compression, dedupe sharing, asset apply/restore/
finalize, prune, and space reporting against actual Btrfs (not the skip path).

## Per-engine round-trips (Balanced / 1080p, level 3)

`restore OK` means a full-tree `sha256sum` of every file matched byte-for-byte
after `assets restore`, and no `.bgc-assets-backup` remained.

| Engine | Game | Plan (count) | Apply (logical before→after) | Backup files | Restore OK |
| --- | --- | --- | --- | --- | --- |
| Godot | Pathogenic | 1 PCK | 1,440,757,360 → 1,050,593,904 (37.1%) | 2 | yes (56 files) |
| Godot | Slay the Spire 2 | 1 PCK | 1,901,047,880 → 1,476,747,784 (28.7%) | 2 | yes (221 files) |
| Godot | Brotato | 1 PCK | 0 changed (`NO_GAIN`) | 0 | yes (5 files) |
| Unity | Core Keeper | 33 | 254,393,711 → 14,341,070 | 66 | yes (1,560 files) |
| Unity | Rain World | 60 | 36,629,017 → 22,965,042 | 120 | yes (31,929 files) |
| GameMaker | How Many Dudes | 18 | 5,382,705 → 3,721,572 | 36 | yes (134 files) |
| Unknown (.NET/MonoGame) | Carrion | 1 | 46,213,120 → 46,176,832 | 2 | yes (525 files) |
| Unknown (.NET/MonoGame) | Celeste | 5 | 272,530,848 → 206,213,568 | 10 | yes (1,259 files) |
| Unreal | Calm Down, Stalin | 0 | no-op (packed textures, no writer) | 0 | yes (2,962 files) |
| RE Engine | Monster Hunter Wilds | 0 | read-only (not applied) | — | — |

Notes:

- Godot `godot-texture-audit balanced`: Brotato `NO_GAIN`; Slay the Spire 2
  `CANDIDATE 28.7403%`; Pathogenic `CANDIDATE 37.1477%`. The planner counts a
  PCK as a candidate even when the rewrite cannot beat the original; Brotato
  shows the apply gate correctly declines it.
- Unity/Unreal packed textures have **no writer**; only loose images/DDS and
  Godot PCK are rewritten. `Calm Down, Stalin` therefore changed nothing and is
  recorded as the honest Unreal result in this library.
- RE Engine (Monster Hunter Wilds) exposes 34 packed containers, 0 supported
  loose candidates.

## Read-only candidates not applied (larger games)

| Engine | Game | Plan (count) | Logical before→after |
| --- | --- | --- | --- |
| Unknown/Godot-like | Noita | 53 | 967,063,329 → 415,886,118 |
| Unknown | Teardown | 52 | 106,467,636 → 42,696,274 |
| GameMaker | FAITH | 3 | 5,882,333 → 4,953,590 |
| Unreal | SpaceEngineers2 | — | plan exceeded 20 min (42 GB loose); not completed |

## Core lossless pipeline

- `bgc-native compress 1` on Brotato reported `Compressed 5 files`;
  `dedupe` checked 34 candidate ranges, 0 rejected, scratch cleaned.
- `bgc-native compress 1` / `3` on CloverPit reported `Compressed 216 files`.
- Unprivileged confirmation of real compression via FS_IOC_FIEMAP:
  `Brotato.pck` has 324 extents with the ENCODED flag.
- `bgc-native measure` and `compsize` need root on this kernel (SEARCH_V2 is
  privilege-gated) and correctly report `unavailable`/`EPERM`, never a fake zero.

## Bug found and fixed

`docs/SPACE_MEASUREMENT.md` documents:

```sh
python3 tools/space-delta.py capture "$GAME" --backend ./bgc-native --output /tmp/bgc-before.json
```

This failed with `extent_measurement.status = "unavailable"`,
`reason = "FileNotFoundError"`. Cause: `--backend` used `argparse type=Path`,
which normalizes `./bgc-native` to the bare name `bgc-native`; `subprocess` then
searched `$PATH` instead of the working directory. The Python tests mocked
`subprocess` with absolute paths, so they missed it.

Fix (`tools/space-delta.py`): keep the argument as the literal string and use
`os.fspath()` so relative paths resolve from the working directory as
documented. Regression test
`test_cli_resolves_documented_relative_backend_path` runs the real CLI with a
relative `--backend ./fake-native` from a temporary directory.

## Caveats

- No game was launched; lossy texture output compatibility remains unvalidated.
- Logical before/after bytes are not filesystem free-space savings.
- The restore copies (retained during apply) are removed by `restore`; finalize
  was not used, so no permanent asset change was left in place.
- All tested games were on Btrfs; ext4 asset-writing was not exercised here.
