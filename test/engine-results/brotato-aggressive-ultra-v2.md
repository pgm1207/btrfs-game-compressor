# Brotato aggressive Ultra Performance v2

User confirmed the 22.05 kHz/PCM8 v1 pack plays/sounds excellent and requested
stronger audio and texture reduction. Ultra Performance limits were tightened
consistently in loose assets and Godot transforms; other profiles are unchanged.

- Sounds: at most 11,025 Hz, signed PCM8, original stereo retained. IMA disabled.
- Textures: at most 640 pixels on the longest edge, 4-bit-equivalent RGB.
  Alpha channel retained (resampled only for resized images); logical dimensions
  and flags preserved. Non-shrinking/unsupported textures stay unchanged.
- Loose-image JPEG target: 60 (not used on Brotato's GDST/WebP textures).
- Fonts, languages, music and executables untouched by these new stages.

Generated fresh from the retained music-only pack, not from previously degraded
WAV/textures. Sources and candidates in `/mnt/storage/Games/bgc-test2/`:

| Stage | Pack bytes | Logical saving from preceding stage |
| --- | ---: | ---: |
| `Brotato.pck.pre-wav-v2` | 88,554,018 | — |
| `Brotato.pcm-11025-ultra-v2.pck` | 72,608,264 | 15,945,754 |
| `Brotato.aggressive-ultra-v2.pck` | 64,625,178 | 7,983,086 |

Relative to the user's last playtested PCM-only pack (74,712,700 bytes), the
new bundle saves another 10,087,522 logical bytes (9.62 MiB). These are not
physical disk measurements; recovery copies consume additional storage.

166 sounds reduced; two unchanged. Independent PCM verification checked every
entry MD5, rate/format/channel metadata, loop scaling and output frame counts.
Maximum duration rounding: 0.272 ms. 956 textures reduced, including 48 resized;
305 unchanged. Separate Pillow/PCK validation checked every entry MD5, decoded
dimensions, 4-bit RGB grid, logical sizes/flags and unchanged alpha for images
that were not resized. All non-texture bytes match the checked PCM-only stage.

78 unit tests passed. The aggressive bundle was staged, recompressed with
Zstd level 1, then atomically installed while Brotato was closed. Post-install
independent texture/PCK verification passed. Installed SHA256:
`54b634fa09f573a4b12b5ba64e75b215269ca1f8bdd648c5a5a05c9b7bbfe0cb`.
In-game loading, atlas layout and listening validation of v2 remain pending.

Subsequent user manual test PASSED: game works; reduced main-menu textures are
visible but acceptable as the Ultra Performance reference. Keep these settings.

Privileged Btrfs extent measurement subsequently became available. Final
installed directory references 90,562,700 compressed physical data bytes;
its logical file sizes sum to 144,827,895 bytes. Installed pack references
55,099,392 physical bytes, with 64,625,178 logical bytes. Final filesystem
dedupe checked 34 ranges, rejected none, and produced no measured reduction
in the directory's unique referenced physical extent bytes. This does not
include Btrfs metadata overhead or backups outside the installed directory.

Playtested rollback pack: `Brotato.pck.playtested-pcm-22050`.
Original unmodified and music-only recovery packs remain retained as well.

```sh
python3 test/verify-godot3-pcm.py \
  /mnt/storage/Games/bgc-test2/Brotato.pck.pre-wav-v2 \
  /mnt/storage/Games/bgc-test2/Brotato.pcm-11025-ultra-v2.pck 11025
python3 test/verify-godot3-textures.py \
  /mnt/storage/Games/bgc-test2/Brotato.pcm-11025-ultra-v2.pck \
  '/mnt/storage/Games/SteamLibrary/steamapps/common/Brotato/Brotato.pck' 640 4
```
