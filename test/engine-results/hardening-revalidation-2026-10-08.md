# Hardening revalidation — 2026-10-08

Execution was explicitly authorized. Builds/tests ran in isolated target
directories (`/tmp/opencode/bgc-target-*`); the repository `bgc-native` and the
frozen installed-run backend were **not** rebuilt or replaced. No installed game
file was modified.

## Automated suites

| Configuration | Command | Result |
| --- | --- | --- |
| default | `cargo check --all-targets` | ok |
| default | `cargo test` | 159 unit + 9 filesystem passed |
| `development-audits` | `cargo check --all-targets --features development-audits` | ok |
| `development-audits` | `cargo test --features development-audits` | 158 unit + 9 filesystem passed |
| Python | `python3 -m unittest discover -s test -p 'test_*.py'` | 16 tests OK |
| Shell | `./test/smoke.sh` | 483 passed / 0 failed |

The one-test delta is the default-only guard test (`#[cfg(not(feature =
"development-audits"))]`). The default binary prints `Development reader routes:
disabled`; the feature binary prints `compiled in`.

## Real read-only audits

- **Carrion XNB** (`xnb-texture-audit`, all 193 `.xnb`): 62 `METADATA_ONLY`,
  123 `COMPRESSED_PAYLOAD_NOT_DECODED`, 8 `CUSTOM_OR_UNSUPPORTED_ROOT_READER` —
  reproduces the 2026-10-07 tally.
- **Cocoon Unity bundles** (`unityfs-texture-inventory`, six Addressables
  `*.bundle`, rc=0): node/block inventory, per-object spans, supported Texture2D
  fields and shape status emitted; exercised the shared `unity_tree.rs` /
  `unity_serialized.rs` changes.

## Detached export (new anonymous-publication path)

Four Carrion artbook pages were copied to scratch and exported at edge 1024:

| File | Original | Candidate | Logical reduction | PSNR black / white |
| --- | ---: | ---: | ---: | --- |
| `page000.xnb` (BC3) | 4,194,389 | 1,048,661 | 3,145,728 | 50.69 / 49.47 |
| `page001.xnb` (BC1) | 2,097,237 | 524,373 | 1,572,864 | 36.18 / 35.97 |
| `page015.xnb` (BC1) | 2,097,237 | 524,373 | 1,572,864 | 37.68 / 37.67 |
| `page031.xnb` (BC1) | 2,097,237 | 524,373 | 1,572,864 | 39.88 / 39.52 |

- Source SHA-256 values unchanged before/after (all four).
- Outputs created with owner-only mode `0600`; no destination replacement.
- `test/verify-xnb-export.py --pixels` (Pillow, independent of `image_dds`)
  confirmed identical target/flags/reader/root prefix, format IDs, exact mip
  lengths, no trailing data, and 2048×2048 → 1024×1024 shapes.

These are seven-plus detached lossy exports and structural/pixel checks, **not**
installed savings, a 130 MB claim, or runtime proof. Atlas references and game
acceptance of the resized book pages are unverified.

## Report schema 3 end-to-end

`tools/asset-opportunity-report.py --backend <feature> --xnb-backend <feature>`
produced a schema-3 report for Carrion: production planner total 0, separate
experimental XNB section 60 candidates / 97,517,568 theoretical bytes with
`70 / 70` audited/header files and `ok` status. Research bytes stayed out of the
production total. Nonzero exit still occurs if the experimental section is
incomplete.

## Caveats

Passing suites and copied-file checks are local development results, not runtime
certification, release artifacts or physical-savings measurement. The publication
path still needs commit-point/disposable-directory edge fixtures, and no
Sprite/SpriteAtlas/PPtr consumer or installed writer exists.
