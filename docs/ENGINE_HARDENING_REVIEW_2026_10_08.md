# Review of the gated engine/export increment

Baseline: `859cb4d` (`feat: add gated engine audits and detached XNB texture
export`). Review date: **2026-10-08**. The baseline arrived committed with a clean
working tree. Its recorded checks were read, **not rerun**. The historical
[Carrion trial](../test/engine-results/carrion-xnb-export-2026-10-07.md) now supplies
independent structural/pixel evidence for seven copied-file exports, including
the BC1 transparency correction. It is not runtime proof or installed savings.

## Improvements retained

- Default-off route boundary; no Bash/automatic/installed dispatch for drafts.
- Exact v5 desktop root subset, allowlisted reader names, no assembly execution.
- Byte-identical no-change rebuilding before lossy resizing; original prefix,
  codec and reader identity retained, strict logical reduction required.
- Corrected BC1 one-bit alpha handling with a re-decoded mask check.
- Independent XNB/Pillow checker and separate theoretical opportunity section.
- Original/frozen backends, recovery data, release artifacts and paused journal
  remain distinct from research builds and disposable copies.

## Source-level gaps addressed locally

| Finding | Local draft change | Deferred proof |
| --- | --- | --- |
| Candidate reparsing wrote a public named temporary then reopened it; also called same-parser checking “independent” | One bounded parser can read descriptor metadata or immutable snapshots; captured source inventory must agree; candidate reparsed in memory; terminology corrected | Both build states, descriptor/snapshot parity and malformed spans |
| Final destination became visible before writes finished; error cleanup removed a pathname after creation | Anonymous same-directory `O_TMPFILE` staging; synced no-replace `linkat` publication; no path cleanup; postcommit durability error retains complete artifact | Destination/source races, interruption, unsupported filesystem/proc and directory-sync failures |
| Large snapshot reads and BC1 block correction lacked useful cancellation boundaries | Chunked descriptor reads, per-row alpha checks, codec-boundary cancellation | Synthetic cancellation and source-change fixtures; codec internals still non-interruptible |
| Export retained full decoded source storage and cloned RGBA/BC buffers unnecessarily | Move owned decoded/encoded buffers, scope source RGBA to resizing, drop candidate RGBA before publication; exact decoder shape/byte checks retained | Codec/alpha regressions and deferred memory measurement; no measured peak-memory claim |
| Optional XNB parsing/timeout could invalidate successful production estimates | Separate research result/status/coverage/errors; preserve production row; incomplete research still exits nonzero | Fake-backend failures, partial results, timeout and resume contracts |
| Theoretical XNB estimate trusted dimensions without source header/mip-span agreement | Exact singleton records, ASCII numbers, source-size/header/root/shared/mip checks; schema 3 rejects older resume records | Malformed/duplicate records and profile-flag fixtures |
| Independent checker accepted custom assemblies after a known reader-class prefix; rejected unused nonroot reader versions | Independent full qualification allowlist; active root version checked; profile flag 1 accepted; bounded non-symlink regular-file snapshots | Qualifier, nonroot version, truncation, flags and preservation fixtures |
| An unknown common Unity tree string could hide a malformed later hierarchy/local string | Structural/local-string pass first; typed budget skips; malformed/unsupported/error statuses separated; parse interruption propagated | Mixed malformed/unsupported/budget cases in both endians and existing inventory regressions |
| Unity tree subtree-end search was quadratic; skipped array fields still constructed unused paths | Linear bounded-depth stack; no path construction for uncaptured elements; captured path length checked before allocation | Old field/span/alignment contracts and bounded work fixtures |

## Implementation milestones, not support promotion

1. Detached XNB publication is now a source-level complete-or-absent design,
   rather than exposing an incomplete destination. Its filesystem contract is
   documented in [ENGINE_EXPORT_GATES.md](ENGINE_EXPORT_GATES.md).
2. Optional research reporting has an independent failure boundary and explicit
   partial coverage. Theoretical bytes remain excluded from production totals.
   Schema-2 reports are not migrated or overwritten; new runs use schema 3.
3. Mixed Unity tree diagnostic handling and a bounded
   [reference/ownership contract](UNITY_REFERENCE_OWNERSHIP_CONTRACT.md) prepare
   the next consumer-reader milestone without guessing stripped schemas.

## Validation performed (2026-10-08, execution authorized)

Compiled in isolated target directories; the repository `bgc-native` and the
frozen installed-run backend were **not** rebuilt or replaced.

- **Default build:** `cargo check --all-targets` and `cargo test` pass;
  **159 unit + 9 filesystem** tests.
- **`development-audits` build:** `cargo check --all-targets --features
  development-audits` and `cargo test` pass; **158 unit + 9 filesystem** tests
  (one delta is the default-only guard test).
- **Python:** `python3 -m unittest discover -s test -p 'test_*.py'` → **16 tests
  OK**, including the new independent-verifier and report failure/coverage tests.
- **Shell:** `./test/smoke.sh` → **483 passed / 0 failed**.
- **Default route guard:** the default binary reports
  `Development reader routes: disabled`; the feature binary reports `compiled in`.
- **Real game read-only audits:** Carrion `xnb-texture-audit` over all 193 `.xnb`
  files reproduced the recorded tally (62 `METADATA_ONLY`, 123 compressed-opaque,
  8 unsupported roots). Cocoon `unityfs-texture-inventory` ran cleanly on six
  real Addressables bundles (rc=0), exercising the shared Unity changes.
- **Detached export (new publication path):** four Carrion artbook pages copied to
  scratch, exported at edge 1024, source hashes unchanged, outputs mode `0600`.
  Independent `test/verify-xnb-export.py --pixels` (Pillow) confirmed identical
  retained prefixes, correct shapes and display PSNR 36.0–50.7 dB.
- **Report schema 3 end-to-end:** `tools/asset-opportunity-report.py` produced a
  schema-3 report for Carrion with the separate XNB section (60 candidates,
  97,517,568 theoretical bytes, `70 / 70` audited/header, `ok`), production
  planner total kept at 0 and separate from the research bytes.

## Remaining gates

Still to do before support promotion: runtime/visual checks on disposable game
copies; independent reconstruction for VPK/VTF/GameMaker/Godot/IoStore; Unity
Sprite/SpriteAtlas/PPtr consumer readers and an exact no-change recipe; and
privileged physical-savings measurement. Source-intent tables above are not
measured performance, snapshot security or tested safety promises.
