# Compatibility-first shared policy and copy trials

## Scope

No per-game resizing recipes or screen-inch-based guesses. Native remains the
byte-preserving default (Btrfs Zstd and deduplication). Explicit lossy profiles
use a conservative shared policy only on supported formats; unknown engines,
serialized textures, encrypted layouts and proprietary codecs stay untouched.
This is safe coverage of the library, not a claim of universal asset rewriting
or guaranteed visual fidelity. Display resolution is not changed by this tool.

The earlier 512-pixel/atlas guard trial remains historical evidence. Current
production calls additionally enforce a relative reduction budget:

- Texture edges retain approximately half of original dimensions or more.
  Requested profile caps are soft and cannot override this floor.
- Known original/logical dimensions anchor the budget to avoid cumulative
  degradation on repeated applies. Godot 4 missing that size reference is skipped.
- RGB quantization retains at least 7 bits/channel; alpha is retained.
- Small/thin textures and known/referenced atlases remain protected.
- Loose asset backups, savings gates, format verification and unsupported-format
  skips remain in place. Packed PCK has no per-file backup (Steam verify recovery).

These shared rules run for every lossy tier in the normal asset pipeline and
Godot exports; no game identifiers are used in the sizing policy.

## Mech Havoc

Source: `/mnt/storage/Games/SteamLibrary/steamapps/common/MechHavoc`.
New trial copy: `/mnt/storage/Games/bgc-mech-havoc-compat-20261002/game`.
Source was never modified. Copy created using `cp -a --reflink=auto`.

Inventory: Unity (`UnityPlayer.dll`), serialized `.assets`/`.resS`, small UnityFS
localization bundles and FMOD resource audio. These are not safe texture rewrite
targets for the current backend. The asset pipeline reports zero supported
rewrites instead of guessing how to edit Unity resource offsets/references.

Commands run on the copy using the rebuilt production backend:

```sh
./bgc-native assets apply ultra-performance 3 /mnt/storage/Games/bgc-mech-havoc-compat-20261002/game
./bgc-native compress 3 /mnt/storage/Games/bgc-mech-havoc-compat-20261002/game
```

- Original manifest: 357 files, 4,761,730,125 logical bytes.
- After asset apply and Zstd: all 357 file lengths and SHA256 hashes match the
  original manifest; no files added or removed.
- FIEMAP confirms encoded/compressed extents in 349 files. This is evidence that
  compression was actually applied, **not** a physical-byte measurement.
- Initial unprivileged `measure-bytes` failed with EPERM. The user subsequently
  authorized read-only privileged measurement; the measured values are below.
  No credentials were saved to project files. `du`/FIEMAP lengths are not
  substituted for compressed Btrfs extent byte counts.

| Scope | Unique referenced physical data bytes | Logical file bytes |
| --- | ---: | ---: |
| Installed Mech Havoc (already compressed) | 576,727,490 | 4,761,730,125 |
| Trial copy after Zstd level 3 | 465,111,125 | 4,761,730,125 |

The copy uses **90.2323% less physical data than its logical file sizes**. The
installed source was already compressed, so the additional referenced-footprint
improvement is **111,616,365 bytes (19.3534%)**, not another 90%. Physical totals
exclude filesystem metadata. Copies/snapshots can retain or share other extents;
these are scope-specific referenced bytes, not a filesystem free-space delta.
Machine-readable results: `mech-havoc-lossless-2026-10-02.json`.

To measure the trial's unique referenced physical data bytes:

```sh
sudo ./bgc-native measure-bytes /mnt/storage/Games/bgc-mech-havoc-compat-20261002/game
```

Output is `disk_bytes|raw_extent_bytes|referenced_bytes`. A lossless reduction
relative to logical bytes is not the same as newly freed filesystem space, and
original/candidate copies may coexist or share extents.

## Slay the Spire 2 regression

New trial: `/mnt/storage/Games/bgc-slay-conservative-20261002/SlayTheSpire2.pck`.
Started from the original installed 1,901,047,880-byte pack.

- Ultra Performance with shared guards: 1,209,911,720 logical bytes (36.35%
  reduction), 1,108 changed textures. Less aggressive than previous candidates.
- Second apply: NO_GAIN, zero changed textures.
- Independent verification:

```sh
python3 test/verify-godot4-textures.py \
  '/mnt/storage/Games/SteamLibrary/steamapps/common/Slay the Spire 2/SlayTheSpire2.pck' \
  /mnt/storage/Games/bgc-slay-conservative-20261002/SlayTheSpire2.pck \
  640 --conservative
```

PASS: hashes, unchanged bytes, logical dimensions, codecs, mip chains and
relative half-size budget. Manual visual testing is pending. Existing prior
candidates were not overwritten or deleted. No games were launched or killed.

## Automated tests

93 unit tests and 433 smoke checks pass. Tests cover shared relative limits,
stable original-size references, missing-reference skips, small/thin protection,
atlas metadata resolution, loose assets, and v1/v3/v4 packing. Subsequently all
**6 real-Btrfs integration tests** passed with `BGC_TEST_BTRFS_DIR` pointing at
new fixtures, without running the test suite as root. The added main-pipeline
test exercises all seven profiles (including Native/Lossless), protects small,
thin, atlas and unknown serialized files, and verifies repeat-apply stability
and restoration. The existing 854px fixture expectation was corrected to the
current 640px cap; it had only been hidden by disabled real-filesystem testing.
`make native` rebuilt the production static backend.
