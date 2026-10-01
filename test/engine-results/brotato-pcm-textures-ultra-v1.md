# Brotato PCM + GDST texture candidate

The installed game is still the working music-only pack (88,554,018 bytes).
Brotato was running during this work; no installed files were changed.

| Stage | Logical pack bytes | Incremental saving |
| --- | ---: | ---: |
| Working music-only source | 88,554,018 | — |
| PCM8 at 22.05 kHz | 74,712,700 | 13,841,318 |
| GDST RGB quantization / resizing | 69,207,650 | 5,505,050 |

Combined incremental saving: 19,346,368 bytes (18.45 MiB). These are logical
sizes, not physical disk savings. Privileged Btrfs extent measurement was
unavailable (`sudo -n` requires authentication). Backups/candidates occupy
additional space until recovery copies can safely be retired.

Texture candidate: `Brotato.pcm-textures-ultra-v1.pck` in
`/mnt/storage/Games/bgc-test2/`. Source is `Brotato.pcm-ultra-v1.pck`.
583 of 1,261 GDST textures shrink; 678 stay byte-identical. 24 changed textures
are resized to at most an 854-pixel longest edge. RGB channels are quantized to
5-bit-equivalent precision then encoded as lossless WebP. Alpha is untouched
for images that are not resized. Resizing necessarily resamples alpha too.
No codec, texture flags, logical size, resource name, atlas references, or
other pack entries change. WebP's Godot `WEBP` prefix is retained. Unsupported
raw/mipmapped layouts fail closed. Any non-shrinking rewrite is discarded.

Logical/custom dimensions follow Godot 3's `StreamTexture::_load_data/load`
implementation in `scene/resources/texture.cpp`. The GDST header's physical
dimensions are changed only for resized textures; custom dimensions retain
the original logical size (including existing overrides).

Checks: 78 unit tests passed. `test/verify-godot3-textures.py`, using Pillow and
a separate Python PCK reader, verified all MD5s, unchanged non-texture assets,
physical image dimensions, retained logical sizes/flags, RGB precision and
unchanged alpha for the non-resized images. Native export verification also
compares transformed payload bytes to its generated buffers. Write-efficiency
is gated before creating/writing the v1 pack. Existing outputs are never replaced.

The PCM-only pack is separately staged with Zstd level 1 at
`/mnt/storage/Games/bgc-test2/pcm-install-v1/Brotato.pck` and independently
verified against the installed pack. Install/test PCM first; only install the
texture bundle after PCM loading and sound playback pass the user's manual
test. Neither candidate has passed in-game validation yet. IMA remains disabled.
Fonts and executables remain intact; no language support is removed.
