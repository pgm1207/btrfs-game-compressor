# Hades experiment results — 2026-10-01

Raw JSON reports from the disposable-copy experiments summarized in
`../HADES_EXPERIMENTS.md`. The large asset copies were removed after these were
saved; the numbers here are not reproducible from these files alone.

- `hades-fmod-guarded-*.json` — conservative/balanced FMOD exports on four small
  banks, with sampled `fmod-audit` timings.
- `hades-fmod-large-*.json` — VO.fsb and Music.bank exports with audits.
- `hades-storage-*.json` — full 138-package pass; baseline, physical-larger
  result, and retained backup footprint.
- `hades-storage-selected.json` — per-package physical comparison, the 13
  rejected packages, and the 125-package selected footprint.
- `storage-levels-*.json` — ZSTD level 3/6/9 physical sizes and warm read+hash
  timings on representative copies.

All measurements are data-extent footprints on this machine only. They exclude
filesystem metadata, snapshots and free-space effects, and do not establish
in-game compatibility or performance.
