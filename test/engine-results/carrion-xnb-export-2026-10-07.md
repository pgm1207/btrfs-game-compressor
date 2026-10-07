# Carrion XNB detached export trial — 2026-10-07

The `development-audits` build inspected all 193 Carrion `.xnb` files on
temporary copies. It reported 62 strict uncompressed v5 Texture2D roots totaling
130,061,462 source bytes, 123 compressed payloads totaling 7,905,143 bytes that
remain opaque, and 8 unsupported root readers totaling 28,027 bytes. Among the
62 strict textures, 58 are 2048×2048 single-mip BC1, 2 are 2048×2048 single-mip
BC3, and 2 are small 64×64 Color textures. Reader names without an assembly
suffix occur in real files and are now admitted only when they exactly match
MonoGame's built-in Texture2DReader name.

The read-only opportunity reporter later ran the existing production planner
plus the development XNB audit against Carrion. The production planner found
**0** eligible logical bytes; the separate experimental XNB section found 60
single-mip BC textures with **97,517,568 theoretical logical bytes** at a 1024
edge. This is a payload-size calculation across both book applications, not an
export or game compatibility claim. The report is under
`/tmp/bgc-carrion-opportunities-2026-10-07/`.

One BC3 artbook page was copied to `/tmp/bgc-carrion-xnb-trial-2026-10-07/` and
passed through the new **development-only detached exporter** at a 1024-pixel
maximum edge. The installed source was hash-checked before and after and was
unchanged. No game was launched and no installed file was replaced.

| File | Original | Candidate | Logical reduction |
|---|---:|---:|---:|
| `CarrionArtBook/Content/page000.xnb` | 4,194,389 bytes | 1,048,661 bytes | **3,145,728 bytes (75.0%)** |

Three further BC1 artbook pages (`page001`, `page015`, `page031`) were copied to
`/tmp/bgc-carrion-xnb-batch-2026-10-07/` and exported at the same edge. Each
went from 2,097,237 to 524,373 bytes, saving 1,572,864 bytes. The **four-page
artbook sample** saved 7,864,320 logical bytes. Three comic book pages (`page000`,
`page008`, `page016`) were also copied and exported, saving another 6,291,456
bytes. The **seven-page sample** saved 14,155,776 logical bytes. Independent
structural checks passed for each, and the installed originals retained their
hashes.

The independent Python checker `test/verify-xnb-export.py` parsed both XNBs and
confirmed identical target/flags/reader/root prefix, BC3 format ID 6, exact mip
payload lengths, no trailing bytes, and 2048×2048 → 1024×1024 dimensions.
Rust also checked a byte-identical no-change rebuild, then decoded and
re-parsed the candidate before creating the destination.
An optional independent Pillow DDS decoder compared the seven output images to
Lanczos-downscaled source pixels. The two mostly transparent BC3 covers measured
49.5–52.8 dB after compositing over black/white. The BC1 artbook pages measured
36.0–39.9 dB; the comic BC1 pages measured 35.4–39.3 dB. These measurements check
decode shape and visible pixel drift against a downscaled reference; they do not
establish that shrinking either book is acceptable in the game.

The first BC1 encoding pass dropped one-bit transparency. `page016` in the comic
book exposed it: display PSNR over white was 29.0 dB. The exporter now restores
BC1 transparent blocks and verifies the re-decoded mask against the resized
pixels. The corrected detached `page016` output measured 35.4 dB over white.
The earlier BC1 trial artifacts remain in `/tmp` for comparison; the corrected
ones have `-alpha.xnb` suffixes. Installed files were never changed.
The source/candidate SHA-256 values were respectively
`b49b243bcba8b4199882ca69b7f2b6bbd6a0795d909b05997a00e4ebe4599acc`
and `f137dadc7a27ebf66acd5b74ef482461bd4a0dda20016d51d5083d37d87a981c`.

This is **seven detached lossy exports**, not a 130 MB savings claim. The other
pages were not exported, runtime artbook layout and visual quality were not
checked. Seven source and exported files were copied onto a scratch Btrfs
directory mounted with `compress=zstd:1` and measured after `fsync`: allocated
bytes fell from **18,903,040 to 4,747,264**, a **14,155,776-byte** reduction.
This is scratch-file allocation (`st_blocks`), not measured space reclaimed by
modifying an installed game. The development command
is unavailable in normal builds and has no installed apply route. Follow-up
requires game-copy visual checks and broader representative validation before
beta support.

Source format evidence: MonoGame's
[Texture2D content writer](https://github.com/MonoGame/MonoGame/blob/develop/MonoGame.Framework.Content.Pipeline/Serialization/Compiler/Texture2DContentWriter.cs)
and [built-in reader registration](https://github.com/MonoGame/MonoGame/blob/develop/MonoGame.Framework/Content/ContentTypeReaderManager.cs).
