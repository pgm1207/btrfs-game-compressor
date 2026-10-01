# Godot desktop-library trial and corrected physical measurements

Historical trial report: Slay the Spire 2 was subsequently reinstalled by the
user; its original 1,901,047,880-byte pack is present again. Provisional Godot
exports and recovery copies were deleted at the user's request. The measurements
below describe the earlier trial, not the current installed Slay pack.

Library: `/mnt/storage/Games/SteamLibrary/steamapps/common`.
Ultra Performance reference: manually validated Brotato v2 (11,025 Hz PCM8,
640-pixel texture cap, 4-bit-equivalent RGB). The asset cap does not force the
game's rendering resolution; it is not a guarantee of 480p texture resolution.

## Brotato

All MB values below are decimal, matching the file explorer.

| Pack stage | Logical bytes | Incremental logical reduction |
| --- | ---: | ---: |
| Original | 147,912,168 | — |
| Music/Vorbis | 88,554,018 | 59,358,150 |
| PCM8 / 11.025 kHz | 72,608,264 | 15,945,754 |
| GDST textures | 64,625,178 | 7,983,086 |

Assets shrank by 83,286,990 bytes (56.31%). Other installed files total
80,202,717 logical bytes and remain intact. Thus the whole installation's
logical size fell from 228,114,885 to 144,827,895 bytes (36.51%).

Privileged `measure-bytes`/`measure-file` use Btrfs extent items' compressed
disk_num_bytes, counting each referenced physical extent once within the
requested scope. Final installed folder: 90,562,700 physical data bytes;
pack alone: 55,099,392. Physical measurement excludes filesystem metadata.
Do not compare these values to `du`, FIEMAP logical lengths, or filesystem-wide
free-space deltas as though they were the same measure.

Current logical-minus-physical difference: 54,265,195 bytes for the directory,
9,525,786 for the pack. This is not a measured incremental compression gain from
the original install: no reliable original physical whole-directory baseline
was retained. Today the original pack backup references 143,503,360 physical
bytes, but that recovery copy is not the historical installation baseline.

Final Brotato filesystem dedupe: 34 candidate ranges, 0 rejected; measured
directory physical delta 0 bytes. Accepted/shared ranges are not claimed as
space saved. Installed pack SHA256 remains
`54b634fa09f573a4b12b5ba64e75b215269ca1f8bdd648c5a5a05c9b7bbfe0cb`.

## Other detected Godot games

| Game | Engine / pack | Original pack bytes | Texture candidate bytes | State |
| --- | --- | ---: | ---: | --- |
| Slay the Spire 2 | 4.5.1 / v3 | 1,901,047,880 | 874,440,584 | Installed, manual playtest pending |
| Pathogenic | 4.7.0 / v4 | 1,440,681,380 | 555,070,356 | Verified export only |
| Sir, We Have an Orc Problem | 4.6.4 / embedded v3, Linux | 24,562,396 | 18,864,976 | Verified standalone export; executable unchanged |
| MOLDRISE | Godot executable marker | — | — | No assets pack located; left unchanged |

Godot 4 audio is not rewritten by the Godot 3 parser. Pathogenic and Sir include
QOA and other audio that remains unchanged. Slay's FMOD banks and fonts remain
unchanged. These candidates are supported-texture passes, not complete
all-format audio/asset optimization.

Godot 4 changes needed for safe exports:

- Preserve GST2 outer logical-size overrides (required by scene/atlas layout).
- Rebuild a complete mip chain for the new physical dimensions, not an invalid
  chain with the original level count.
- Apply the profile's RGB precision to supported pixel encodings, retaining the
  codec and alpha channel; unsupported raw/HDR formats fail closed.
- Support plain PCK v4's v3-compatible layout, confirmed against Godot's pack
  loader. Encrypted, sparse, delta and unknown layouts remain rejected.
- Gate v3/v4 export writes before creating output, then verify changed payload
  MD5s and byte-compare unchanged entries.

Independent `test/verify-godot4-textures.py` checked all source/output MD5s,
unchanged assets, preserved logical dimensions/flags/codecs, every new mip
chain, and decoded PNG/WebP/BC1/2/3/7 output images through Pillow. Changed
textures: Slay 1,997; Pathogenic 1,704; Sir 17. Independent final installed Slay
verification passed after Zstd/dedupe. 80 unit tests passed.

Packed deduplication estimates: Slay 959,598 bytes, Pathogenic 9,119,545, Sir 0.
The first two were below the 5% gain/output-write gate, so no extra pack rewrite
was performed solely for these small gains. Filesystem dedupe is separate.

## Slay the Spire 2 installed trial

Original pack recovery reflink:
`/mnt/storage/Games/bgc-godot-tests/SlayTheSpire2.install-original.pck`.
Installed candidate SHA256:
`7eee8da10631e60b42f24db28a4b4ec9afd6f8e75cf791def34283e7138f1fc1`.

While the game was closed: installed textures, applied Zstd level 1 to all 221
installed files, then ran filesystem dedupe. Dedupe checked 4,491 ranges with
0 rejected. Use assets → optional packed dedup → Zstd → filesystem dedup;
defragment/recompression after filesystem dedup can break sharing.

| Installed folder measure | Before | Final |
| --- | ---: | ---: |
| Logical file sizes | 2,096,907,965 | 1,070,300,669 |
| Unique referenced physical data bytes | 1,401,338,272 | 845,035,908 |

Final physical referenced reduction: 556,302,364 bytes (39.70%). After Zstd but
before filesystem dedupe the folder referenced 849,865,092 bytes, so final
dedupe reduced that figure by 4,829,184 bytes. Final pack references 760,881,152
physical data bytes. Exact machine-readable records are retained in this report
directory as `slay-the-spire-2-trial-measurements.json` and
`brotato-trial-measurements.json`.

IMPORTANT: these are installation-scoped referenced-data reductions, not net
filesystem free-space savings. At measurement time recovery packs/candidate
exports retained original/extra extents outside the install. Those provisional
copies were later cleaned up at the user's request. No games were launched or
killed automatically.
