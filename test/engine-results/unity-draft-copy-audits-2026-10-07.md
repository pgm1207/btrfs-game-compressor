# Unity development reader copy audits — 2026-10-07

Two installed Unity bundles were copied into temporary directories, then inspected
with an isolated `development-audits` build. The installed sources were SHA-256
checked before and after; both remained unchanged. The copies and their audit
outputs were not installed into any game. These are metadata-only development
results, **not texture optimization eligibility or savings estimates**.

| Bundle | Compressed file | Texture2D records | Declared streamed texture bytes | Known mip shapes | Reference obstacle |
|---|---:|---:|---:|---:|---|
| Cocoon UI bundle | 39,795,950 bytes | 92 | 71,466,044 bytes | 89/92 | 89 Sprite records; stream ownership unresolved |
| WHAT THE GOLF scenes bundle | 40,435,214 bytes | 12 | 3,129,712 bytes | 12/12 | 1,631 external references across selected nodes; stream namespace unresolved |

The Cocoon records include 69 BC7 textures declaring 67,698,768 streamed bytes,
14 RGBA32, 6 DXT5 and 3 Alpha8. Every stream range is in bounds of a same-bundle
resource node, with no aliases/overlaps reported, but bounds do **not** establish
resource ownership. The three Alpha8 mip shapes remain unknown to this reader.
The BC7 shape check was added using Unity's
[format IDs](https://github.com/Unity-Technologies/UnityCsReference/blob/master/Runtime/Export/Graphics/GraphicsEnums.cs)
and Microsoft's [16-byte BC7 blocks](https://learn.microsoft.com/en-us/windows/win32/direct3d11/bc7-format).

The WHAT THE GOLF scan skipped 62 selected SerializedFile nodes after reaching
its aggregate decode/copy budget and classified 84 resource nodes as opaque.
Its 12 streamed textures use a namespace the resolver does not recognize. The
1,631 external-reference records are declarations, not resolved ownership.

**Implication:** Cocoon shows substantial packed texture data, but Sprite/UI
geometry and stream ownership are the immediate gates. The sampled scene bundle
offers little texture volume and many unresolved references. Neither should be
used as a first installed writer target from this audit alone. A narrow detached
no-change rebuild and independently checked reference graph are still required.
