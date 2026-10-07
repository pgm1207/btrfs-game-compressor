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

Bundled Unity content (Addressables/asset bundles) **retains type trees**, unlike
the stripped standalone player builds sampled earlier. That means Texture2D
dimensions, format and mip data are **readable in bundles**, where the texture
bytes actually live. This materially changes the Unity outlook:

- A Unity Texture2D writer no longer depends on a stripped-schema fallback for
  bundle content; it depends on bundle node/block rebuild and `.resS` relocation.
- The remaining hard parts are container-side: rebuilding SerializedFile object
  tables and `.resS` stream offsets together, and keeping bundle CRC/hash/catalog
  consistency.

## Honest caveats

- Object payloads stay opaque in this pass: Texture2D pixel data is not decoded
  and the `.resS` extents are not validated here.
- No writer exists. Listing a Texture2D is not permission to rewrite it.
- No in-game playtest was performed.
