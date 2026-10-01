# Brotato Ultra Performance PCM candidate

Source: working music-optimized pack, 88,554,018 bytes. Installed source is
unchanged. Export: `/mnt/storage/Games/bgc-test2/Brotato.pcm-ultra-v1.pck`,
74,712,700 bytes. Incremental logical saving: 13,841,318 bytes (13.20 MiB).
Physical allocation is unmeasured; backups/exports are not net savings.

166 of 168 samples become 22,050 Hz signed 8-bit PCM. Two stay unchanged.
Stereo stays stereo; resource class stays AudioStreamSample. IMA remains
disabled. The PCM path preserves original resource headers, internal path,
string table, property identifiers and trailer rather than rebuilding the
container. Missing properties use Godot's inline property-name representation.

Policies match the existing loose-WAV profile limits:

| Profile | Maximum rate | PCM bit-depth ceiling |
| --- | ---: | ---: |
| Ultra Performance | 22,050 Hz | 8 |
| Performance | 32,000 Hz | 16 |
| Balanced | 44,100 Hz | 16 |
| Quality / Ultra Quality | 48,000 Hz | 16 |
| Native / Lossless | unchanged | unchanged |

No upsampling or bit-depth increases. Rate reduction uses a windowed-sinc
low-pass filter, not sample dropping. Loop endpoints are scaled in frames;
loop mode stays unchanged, including reverse/ping-pong. Payload lengths are
kept 4-byte aligned by rounding down the output frame count. Independent
verification measured a maximum duration rounding difference of 0.136 ms.

Checks: 75 unit tests passed; independent Python RSRC parser checked pack entry
MD5s, every unchanged asset, PCM formats, rates, channel metadata, scaled loops
and duration rounding for all 166 changed entries. No game compatibility or
listening test has passed yet. Do not describe this candidate as validated
in-game. Working installed pack remains the restored pre-WAV version.

```sh
python3 test/verify-godot3-pcm.py \
  '/mnt/storage/Games/SteamLibrary/steamapps/common/Brotato/Brotato.pck' \
  /mnt/storage/Games/bgc-test2/Brotato.pcm-ultra-v1.pck
```

Textures, fonts and executables remain unchanged. This is an audio-stage
candidate, not a complete all-assets Ultra Performance implementation.

## Installation update

After the user closed Brotato, installed the PCM-only candidate by atomic
replacement from its Zstd level 1 staged copy. Rechecked installed bytes with
the independent PCM verifier: all 166 changed samples passed. Installed SHA256:
`fcd7d4d9c3149a4669c965940cf5d117353db43c602352be4f679db33c62fe6f`.
Recovery pack: `/mnt/storage/Games/bgc-test2/Brotato.pck.pre-wav-v2`.
User manual loading/sound test is pending; texture candidate is not installed.

Subsequent manual test PASSED: user reports the game works and sounds excellent.
This 22.05 kHz PCM-only pack is now retained as the rollback baseline at
`/mnt/storage/Games/bgc-test2/Brotato.pck.playtested-pcm-22050` while testing the
more aggressive v2 audio+texture bundle.
