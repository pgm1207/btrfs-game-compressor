# Shared audio policy and main-pipeline FMOD trial — 2026-10-02

## Implementation

`audio_policy.rs` defines asset-profile targets for WAV, Godot 3 PCM/Vorbis and
standalone FMOD FSB5 Vorbis. Zstd level does not change audio quality. Native and
Lossless never transcode. Automatic FMOD applies only to standalone `.fsb` and
`.bank` files up to 256 MiB, preserving rates, frames, channels, names, identity,
loops and supported event metadata. Unknown codebooks/offset metadata are skipped.
Samples require at least 5% encoded savings including seek metadata and a decoded
waveform rejection floor (20/20/20/22/24 dB across the lossy tiers). The complete
file must shrink. Main-pipeline backups, checksums and repeat-apply guards apply.

The waveform guard is not a listening test or a gameplay compatibility guarantee.
Embedded Unity `.resource` audio, Unreal packed audio, Godot 4 audio and other
unsupported encoded audio are not rewritten. Unity serialized textures and
Unreal cooked textures/archives still have no production texture writer; engine
audits must not be represented as implemented support.

## Disposable Hades bank copies

Source: `Hades/Content/Audio/FMOD/Build/Desktop/DeathArea.bank`.
Each profile started with a separate copy of the same original. Installed files
were not modified, and no game was launched or killed.

| Profile | Original bytes | Applied bytes | Logical reduction |
| --- | ---: | ---: | ---: |
| Native | 3,838,272 | 3,838,272 | 0% |
| Lossless | 3,838,272 | 3,838,272 | 0% |
| Ultra Performance | 3,838,272 | 3,668,560 | 4.42% |
| Performance | 3,838,272 | 3,750,736 | 2.28% |
| Balanced | 3,838,272 | 3,791,008 | 1.23% |
| Quality | 3,838,272 | 3,838,272 | 0% |
| Ultra Quality | 3,838,272 | 3,838,272 | 0% |

Every profile's second apply was byte-stable. Restore returned each copy to the
original bytes. The installed source's SHA256 was rechecked after the trial.
These are logical reductions, not physical free-space measurements: originals
and test copies remain allocated. Higher-quality tiers safely found no gain.
Test copies: `/mnt/storage/Games/bgc-audio-pipeline-20261002/` (restored originals).
Detailed local diagnostics: `/tmp/opencode/hades-audio-pipeline.json`.

## Validation

- 96 Rust unit tests passed with `BGC_TEST_BTRFS_DIR` enabled, including the new
  FMOD real-Btrfs apply/backup/checksum/repeat/restore test (not a skipped test).
- All 6 filesystem integration tests passed against real Btrfs fixtures.
- 433 shell smoke checks passed; Bash syntax and `git diff --check` passed.
- `make native` rebuilt the static production backend.
- No privileged operations were needed for this test batch.
