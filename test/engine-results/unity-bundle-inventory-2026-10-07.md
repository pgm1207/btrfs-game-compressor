# UnityFS bundle inventory — 2026-10-07

Read-only validation of the new `unityfs-inventory` command against real installed
Unity AssetBundles (Addressables under `*_Data/StreamingAssets/aa/`). Nothing was
modified.

## Method

```
bgc-native unityfs-inventory PATH
```

The command parses a UnityFS bundle, decodes its blocks in memory (bounded by the
existing 2 GiB decoded-work limit), slices each resource node, and summarizes any
SerializedFile node through the standalone metadata reader. It writes nothing.

## Results

**WHAT THE GOLF** scenes bundle
(`.../aa/StandaloneLinux64/specialeventpacknonarcade_scenes_all.bundle`):

```
UNITYFS_BUNDLE|8|579|248|122757521
UNITYFS_TOTAL|248|164|13|2916
```

- 248 nodes, 164 SerializedFiles, 70 `.resS` streams, 14 `.resource` files.
- 164/164 SerializedFiles parsed; all reported **type trees present**.

**Cocoon** texture bundle
(`.../aa/StandaloneWindows64/ui_assets_all_0ad4065fcba30d970433aa6646a1c619.bundle`):

```
UNITYFS_TOTAL|2|1|92|21204
```

- One SerializedFile with **92 Texture2D objects** and **type trees present**.

## Key finding

The **sampled** bundled Unity content retains type trees, unlike the stripped
standalone player builds sampled earlier. This makes type-tree-guided Texture2D
inspection a promising next step, but this metadata-only inventory did not inspect
dimension/format/mip fields or decode pixel payloads. This changes the Unity outlook:

- Bundles with supported readable trees can avoid a stripped-schema fallback;
  bundle node/block rebuild and `.resS` relocation still need implementation.
- The remaining work includes readable object schemas and field spans, reference
  and stream ownership, rebuilding SerializedFile object tables and `.resS`
  offsets together, and keeping bundle CRC/hash/catalog consistency.

## Honest caveats

- Object payloads stay opaque in this pass: Texture2D pixel data is not decoded
  and the `.resS` extents are not validated here.
- No writer exists. Listing a Texture2D is not permission to rewrite it.
- No in-game playtest was performed.

### Research clarification — 2026-10-07 (no new validation)

Unity permits `BuildAssetBundleOptions.DisableWriteTypeTree`; tree presence is
not universal. The reported texture byte totals are serialized object sizes,
not external pixel bytes. See [the next compatibility design](../../docs/ENGINE_COMPATIBILITY_NEXT.md)
for stream ownership, schema, reference and integrity prerequisites.
