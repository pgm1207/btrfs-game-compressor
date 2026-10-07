# Godot texture opportunity in the installed library — 2026-10-07

Read-only measurement to find where texture bytes actually are in the installed
Steam library, using the existing supported writers. Nothing was modified.

## Godot 4 (`.ctex` / GST2) — supported writer

`bgc-native godot-texture-audit balanced PCK` (read-only), largest PCKs:

| Game | PCK entries | Changed | Skipped | Source bytes | Candidate bytes | Saved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Slay the Spire 2 | 12,327 | 243 | 3,242 | 1,901,047,880 | 1,476,747,784 | **424,421,282 (22.3%)** |
| Pathogenic | 3,226 | 141 | 3,085 | 1,440,681,380 | 1,050,517,844 | **390,271,570 (27.1%)** |
| Project P.I.T.T | 133 | 3 | 130 | 96,487,884 | 96,108,924 | 393,138 (0.4%) |
| Cassette Beasts (Godot 3) | 4,816 | 0 | 4,813 | 945,978,407 | 945,978,407 | 0 |
| Others (Godot 3) | — | 0 | — | — | — | 0 |

So the two biggest single-game texture wins in this library are **already
supported** by the Godot 4 writer and are simply not yet applied:
~814 MB combined at the `balanced` profile.

## Godot 3 (`.stex` / GDST) — mostly small sprites

`container-audit` now reports a `.stex` histogram (`PCK_STEXTURE`, added in this
change). Real data:

- Brotato: 1261 single-mip **WebP** `.stex`, max 2070x1080, but the typical file
  is **96x96** (max dimensions come from a few outliers).
- Cassette Beasts: 4625 single-mip WebP `.stex` (max 4000x1920, typical
  **100x100**) plus a handful of multi-mip WebP (7-11 levels).

A direct export check (`godot3-optimize balanced` on Brotato) reports `NO_GAIN`
with 0 textures converted: the small/thin texture policy correctly refuses to
downscale already-small sprites. Multi-mip `.stex` remain skipped (single-level
only), but the bytes involved are small.

Conclusion: the Godot 3 GDST "gap" is not a missing writer for meaningful bytes;
it is small sprites that should not be downscaled.

## Where the remaining texture bytes are

The large remaining pools are **Unity** (serialized/bundle textures) and
**Unreal** (cooked `.uasset`/IoStore), which have no writer yet. Loose DDS is
only two titles (~1.5 GiB total, see the DDS evidence). This is why the
supported writers already capture the biggest tractable wins, and why the next
real texture step is an engine container writer, not more loose-format work.

No installed game was changed. No in-game playtest was performed.
