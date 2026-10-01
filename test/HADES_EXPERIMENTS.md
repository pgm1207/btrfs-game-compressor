# Hades asset/storage experiments — 2026-10-01

Installed game assets were not replaced. All mutation experiments used separate
copies in `native/target`; those copies were removed after the raw JSON was
saved under `test/hades-results/`. Measurements are data-extent footprints, not
total filesystem free space or metadata, and do not establish game
compatibility.

## Installed baseline

- Logical file bytes: 11,899,644,249 (11.08 GiB).
- Privileged physical extent measurement: 11,148,062,521 (10.38 GiB).
- Difference: 751,581,728 bytes (716.8 MiB, 6.32%).

## Lossless packages: physical selection matters

All 138 Hades v7 packages were independently copied, compressed with Btrfs
ZSTD level 6, losslessly LZ4-recompressed, then measured again.

| Stage | Physical data bytes |
| --- | ---: |
| Original copies after ZSTD | 2,033,598,464 |
| All 138 recompressed packages | 2,041,475,072 |
| Selected 125 packages; 13 restored | 2,005,176,320 |

Blind application reduced logical package bytes by 50,000,459 (47.7 MiB), but
**increased physical storage by 7,876,608 bytes**. Selecting only individually
smaller physical candidates instead reduced the live footprint by **28,422,144
bytes (27.1 MiB)**. The accepted set retained 1,757,022,285 bytes of originals
and checksum sidecars, so its net footprint with backups was still larger.

The shell Lossless workflow now attempts per-file physical selection and always
requires a strictly smaller aggregate live footprint; otherwise it restores
originals. A fresh backup set is required to keep rollback scope unambiguous.
Finalization after game testing is still required for potential net savings.

## Conservative FMOD exports

Only candidates passing sparse decoded-waveform guards and per-stream minimum
savings were accepted. Sample ordering, rates, declared frame counts, names,
non-codec metadata and event metadata were preserved. These are logical export
sizes, not additional installed-game or verified physical savings.

| File | Profile | Original bytes | Export bytes | Changed streams |
| --- | --- | ---: | ---: | ---: |
| VO.fsb | balanced | 687,106,752 | 680,858,192 | 2,695 / 19,597 |
| Music.bank | conservative | 266,552,576 | 265,826,592 | 11 / 103 |
| ChaosRealm.bank | balanced | 150,528 | 131,248 | 2 / 3 |
| Tartarus.bank | balanced | 3,672,192 | 3,500,640 | 28 / 156 |
| DeathArea.bank | balanced | 3,838,272 | 3,730,736 | 6 / 58 |
| Enemies.bank | balanced | 2,966,912 | 2,952,336 | 13 / 398 |

Quality-conscious presets save much less than unguarded aggressive transcoding.
The large voice/music tests took about 745/304 seconds of offline processing.
Bounded lewton decoder samples showed similar decode times before/after. This
is **not** an FMOD runtime CPU benchmark or playback, loop or seeking validation;
automatic FMOD replacement remains disabled.

## Filesystem levels

A 100,524,032-byte extent sample of a large Bink animation, Launch.pkg,
Tartarus.bank and a large SJSON file gave these physical sizes:

| ZSTD level | Physical bytes |
| --- | ---: |
| 3 | 96,878,592 |
| 6 | 96,854,016 |
| 9 | 96,837,632 |

Level 9 saved only 40,960 bytes more than level 3 in this sample. Warm read+hash
times were about 67–73 ms; they include hashing and caching, not just decoder
CPU time. Neither timing nor sample savings should be projected onto gameplay
or the entire game.

## Unused-content pruning (applied to the installed game)

`Hades.log` (written by the engine) reports `Preset: high`, GPU
`Intel(R) Graphics (LNL)`, and 2560x1440, and contains **zero** occurrences of
`Using 720p binks`, `Using 720p packages` or `Using BC3 packages`. The engine
therefore loads the full-resolution suites, so the 720p/BC3 fallbacks and the
shipped debug symbols are dead weight.

| Change | Files | Logical | Live-footprint drop |
| --- | ---: | ---: | ---: |
| Debug symbols (`*.pdb/*.ilk`) | 25 | 371,340,612 | 91,090,944 (86.9 MiB) |
| Unused 720p/BC3 suites | 1530 | 3,312,346,792 | 3,212,267,116 (2.99 GiB) |

Live game data fell from 11,148,062,521 to 7,844,704,461 bytes (10.38 GiB to
7.31 GiB, **29.6% smaller**). Symbols reclaim far less than their logical size
because Btrfs ZSTD already compressed them ~4:1; the Bink/LZ4 fallbacks are
already compressed, so 97% of their logical bytes became real physical savings.

### Unused architecture / renderer builds

`Hades.log` also shows the 64-bit Direct3D 11 build running (`direct3d11.cpp`,
`Running Hades in 64-bit`), so the 32-bit build (`x86`), the alternate Vulkan
build (`x64Vk`) and the Vulkan shader suite (`Content/Win/EffectsForgeVulkan`,
which only `x64Vk` references) are unused.

| Change | Files | Logical | Live-footprint drop |
| --- | ---: | ---: | ---: |
| `x86`, `x64Vk`, `EffectsForgeVulkan` | 213 | 78,953,779 | 20,960,111 (20.0 MiB) |

### Final state (backups finalized)

| Stage | Logical | Physical |
| --- | ---: | ---: |
| As installed | 11,899,644,249 | - |
| After the pre-existing ZSTD + dedupe pass | - | 11,148,062,521 |
| After debug-symbol pruning | - | 11,056,971,577 |
| After 720p/BC3 pruning | - | 7,844,704,461 |
| After architecture pruning | - | 7,823,744,350 |
| Now (backups finalized) | 8,137,003,066 | 7,823,744,350 |

Total physical reduction from the installed logical size: 4,075,899,899 bytes
(3.80 GiB, 34.3%). Of that, this session's pruning accounted for
3,324,318,171 bytes (3.10 GiB); the filesystem's own ZSTD + dedupe accounted for
the remaining 751,581,728 bytes (716.8 MiB). The backup tree was finalized, so
the space is actually free (about 3 GB more on the filesystem; snapshots would
retain blocks).

## Bink blocker

The official download page offers players; licensed SDK access is separate.
The public third-party Bink2 encoder archive describes patched game DLLs and
does not establish redistributable, compatible native KB2j encoding. It was
not integrated. A verified compatible encoder or a substantial native codec
implementation is still needed; container inventory is not encoding support.

Sources: https://www.radgametools.com/bnkdown.htm and
https://github.com/marcussacana/Bink2/blob/main/Readme.md.
