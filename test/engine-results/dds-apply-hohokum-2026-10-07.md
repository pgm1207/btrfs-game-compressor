# Loose DDS in-place apply on a disposable Hohokum copy — 2026-10-07

Scope: a **disposable copy** of every `.dds` file under
`Hohokum/assets/` (178 files, 710,563,292 bytes ≈ 678 MiB) on Btrfs under
`~/.cache`. The **installed game was never modified** (its DDS total stayed
710,563,292 bytes). The copy was driven through the real asset pipeline:

```
bgc-native assets plan  ultra-performance 0 COPYDIR        # read-only
bgc-native assets apply-no-backup ultra-performance 1 COPYDIR
bgc-native assets apply-no-backup ultra-performance 1 COPYDIR   # repeat
```

`ultra-performance` caps the longest edge at 640 px (480p). The source textures
are 1920x1080 legacy DXT5, DXT3, DXT1 and uncompressed BGRA, almost all
single-mip.

## Results

| Stage | Logical bytes | Delta |
| --- | ---: | ---: |
| Read-only plan (147 candidates of 178) | 703,460,256 → 82,229,636 | 88.3% reduction |
| Apply pass 1 (whole tree) | 710,563,292 → 89,332,672 | **621,230,620 saved (87.4%)** |
| Apply pass 2 | 89,332,672 → 89,332,672 | **0 — idempotent** |

- Pass 2 produced an identical tree hash
  (`6fcc511655fe6c8c78d9ea5de52d0bcb`), so repeat-apply is a no-op.
- Each replaced texture keeps its codec (DXT5 stays DXT5, BGRA stays BGRA) with a
  regenerated mip chain. Replacement only happens when the rebuilt file is
  strictly smaller.

## Independent verification

Every processed file was re-parsed and its base mip decoded by a separate
read-only pass (`texture-compress-tree 8192`): **178/178 parsed and decoded,
0 failures**. `texture-compress-tree` wrote nothing because no file was further
reducible.

## Honest caveats

- These are **logical file bytes**, not measured physical Btrfs savings and not
  net free-space gain.
- **No in-game playtest was performed.** This is a 2D sprite game; downscaling
  to 480p reduces detail and could affect reading UI, and any engine or data that
  assumes exact texture dimensions is a risk. The profile, atlas/small/thin
  guards and per-file reduction gate limit risk but do not remove it.
- `ultra-performance` (480p) is an aggressive demonstration. `balanced` (1080p)
  would not change these already-1080p textures, because adding a mip chain to a
  texture at the cap would grow it, which the reduction gate refuses.
- Installed-game apply remains **beta** and opt-in; recovery without a retained
  restore copy is Steam verification.

## Commands used

```
# copy (not the installed game)
find "$SRC" -iname '*.dds' -print0 | while IFS= read -r -d '' f; do
  cp "$f" "$COPY/$((i+=1)).dds"
done

./bgc-native assets plan               ultra-performance 0 "$COPY"
./bgc-native assets apply-no-backup    ultra-performance 1 "$COPY"
./bgc-native texture-compress-tree 8192 "$COPY" "$VERIFY"
```
