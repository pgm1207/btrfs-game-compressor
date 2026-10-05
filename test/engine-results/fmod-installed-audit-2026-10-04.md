# Installed standalone FMOD bank audit — 2026-10-04

Read-only structural audit across the 59 Steam installs where the format-aware
inventory found `.bank`/`.fsb` files. This is not an eligibility count for lossy
conversion and does not establish game playback.

- Files seen: 1,444 (about 27.9 GiB by extension inventory).
- Strict standalone bank parser accepted 889 files.
- It rejected 555, mostly strings banks, metadata-only banks, and variants whose
  FSB fields do not match the supported standalone container layout.
- No installed file was written or replaced.

## Disposable-copy pipeline trials

The normal `assets apply balanced 1` and `assets restore` commands were run on
full disposable copies of Bread & Fred, Gladiabots, CARRION, Worms W.M.D, Noita,
Closer the Distance, Hades II, and Hades. For each, the source bank SHA-256
values were recorded, bank auditing passed before/after, restore returned the
copy's bank hashes to baseline, and the installed source hashes remained
unchanged. The pipeline accepted candidates in 7 of 8 games; Closer the
Distance had no accepted reductions. Some installations also had supported
loose images/WAVs or Hades packages, so the pipeline summary is not FMOD-only.
This still is not in-game playback validation.

The in-place asset pipeline already invokes this parser and its bounded stream
decoder, retains unknown banks untouched, checks candidate audio quality/timing,
and keeps a restorable original for supported standalone files. Next validation
should use disposable copies and that existing apply/restore pipeline, not a new
write path.
