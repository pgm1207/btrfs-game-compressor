# Pathogenic Ultra Performance trial — 2026-10-02

Selected because Pathogenic was absent from the existing Zstd ratio database.
This is an **asset export trial**, not a measured Btrfs ratio to insert into
`ratios/games.json`. Installed assets were not changed and the game was not
launched. The game directory was reflinked on the same SSD before testing.

## Baseline and result

- Steam app 3808690, build 24672689; exported Godot 4.7.0, PCK format v4.
- Current installed directory: 2,945,601,969 logical bytes, including artbook
  and soundtrack. This is the current state, not a claim of pristine Steam data.
- Pack: **1,440,681,380 → 786,753,844 bytes**, a 653,927,536-byte reduction
  (**45.39%** of the pack, **22.20%** of the whole directory if replacing only
  the pack). These are logical sizes, not physical SSD/free-space savings.
- 581 `.ctex` textures changed; 2,645 preserved. All 12,248 entry names/order,
  engine/version fields and flags preserved.
- Independent Python directory parser verified source/candidate entry MD5s,
  SHA256 equality for all 11,667 unchanged entries, and smaller changed textures
  retaining GST2 logical dimensions, flags, format and encoding. It does not
  establish visual/gameplay compatibility.
- Repeat with the same profile: **NO_GAIN**, zero changed textures, and no
  repeat output created.

Installed/original PCK SHA256:
`5ac556a889f6d128d6f05bc7728f5b7366334c1feb2890b31b541114a30c1bd8`

Final candidate SHA256:
`eecbbed71190a805e5e23a5b2b3b3433be41528684a2abce5b266cb93c1e8bdb`

## Real failures fixed

1. Initial texture audit refused a scene exceeding the atlas metadata reader's
   4 MiB per-entry limit. The ordinary asset preview counted the pack but did
   not compute packed texture savings. The atlas scan now streams a buffer of
   less than 70 KiB, carries overlaps for complete 4096-byte paths and split
   `AtlasTexture` markers, and uses a 64 MiB per-resource / 256 MiB total read
   budget (including second passes). Path count/text budgets remain bounded.
   This preserves existing literal atlas reference protection rather than
   bypassing it. Compressed or unrecognized atlas declarations are still a caveat.
2. The first candidate's repeat altered 27 BC7 textures for ~19 KB more savings.
   Block-padded original dimensions produced a one-pixel repeat budget decrease.
   Reconstructing BC padding from logical dimensions fixes this; additionally,
   at-cap BC textures are never quantization-only lossily re-encoded. The final
   candidate was rebuilt from the original, not from the repeated output.

## Remaining opportunities and blockers

The original pack contains 1,120,385,476 texture payload bytes, 140,668,409 MP4
bytes, 40,497,013 Ogg resource bytes and 5,433,463 WAV-resource bytes, including
QOA. Godot 4 packed audio and video remain unchanged. Soundtrack/artbook files
remain unchanged; no DLC, language, debug or resolution content was pruned.

No exact Btrfs compression, dedupe or physical measurement was performed:
native FIEMAP measurement needs privileges and `sudo -n` was unavailable.
Keeping original and candidate copies means this trial itself consumes space;
the pack reduction is not a filesystem free-space delta.

## User playtest

Automated validation: 122 unit tests, 7 integration tests with real Btrfs fixtures
enabled, and 433 shell smoke checks passed. The release backend was rebuilt.

Candidate:
`/mnt/storage/Games/bgc-pathogenic-ultra-20261002/pathogenic-ultra-final.pck`

Steam launch option to load the candidate while leaving the installed pack intact:

```text
--main-pack /mnt/storage/Games/bgc-pathogenic-ultra-20261002/pathogenic-ultra-final.pck
```

Remove that option to return to the installed original. Check startup, UI/text,
interactions, environmental textures, transitions, audio and a saved game. The
agent has not tested launching or gameplay; do not label this runtime-verified.
