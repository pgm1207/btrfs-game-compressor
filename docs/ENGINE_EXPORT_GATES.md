# Reader-to-export contracts — installed writers remain disabled

Design date: **2026-10-07**. The [reader drafts](ENGINE_DEVELOPMENT_STATUS.md)
have isolated build, fixture and selected copied-file validation. A bounded XNB
v5 Texture2D **detached development export** now performs a byte-identical
no-change rebuild check before resizing, but runtime compatibility and visual
quality are unverified. No installed writer is enabled by these drafts.

## Shared separation of responsibilities

Keep four separate results:

1. **Storage inventory:** original byte spans, ranges and identities, with explicit
   opaque/budget/error status. No optimization permission follows from this.
2. **Rebuild recipe:** exact understood schema, original retained bytes and every
   size/offset/alignment dependency. Unknown dependencies block a recipe.
3. **Export artifact:** new destination, unchanged source, full structural and
   independent media/reference verification. Never a guessed drop-in replacement.
4. **Installed eligibility:** purpose/reference/integrity/recovery and runtime
   gates satisfied for that operation. No new reader currently reaches this level.

The recipe must include source identity plus exact retained-byte expectations;
file length/mtime alone is not a collision-resistant input identity. Do not use
hashes as permission to follow an untrusted path. No arbitrary script, assembly,
provider, resource constructor or archive name may execute during parsing.

## XNB v5: detached export and remaining gates

Proposed boundary: uncompressed v5 `w`/`d`, allowlisted reader version 0/root
Texture2D, no shared resources and one of the currently inventoried storage maps.
Still verify those maps and assembly-name normalization independently.

The source draft records original reader record/name/version offsets, the reader
table and root-index spans, the 16-byte Texture2D header and every mip length
prefix/payload span. These are **original offsets**, not an editing API.

The development export checks a byte-identical no-change rebuild and must:

- Preserve the header target/profile/version/flags, reader strings and versions,
  shared-resource count and root reader index verbatim, including exact allowed
  assembly qualification. Do not manufacture another reader table.
- Reproduce the complete root serialization and file length; preserve all pixel
  blobs exactly. Require independent reading of the whole resulting root.
- Reject any extra root/shared data rather than emitting only the first apparent
  texture. Never reinterpret v4 numeric surface IDs with the v5 map.

A later resizing export updates width/height, mip count, each mip length and
declared file size together. Surface codec, channel/alpha/color semantics and
profile flags remain unchanged. A retained lower-mip suffix can avoid BC
re-encoding but still changes dimensions/texel usage: it is lossy and may break
atlas/scripting coordinates. Unknown external texture consumers remain blockers
for installed apply. A detached research artifact is not a validated game patch.

## Unity: exact schema and ownership before rebuilding

Start with one exact SerializedFile version/endian/schema and inline storage,
not the entire v17–22 header range. The draft's selected field spans can locate
original bytes, but `PRIMITIVE_BITS` does not establish signedness or purpose.
Required size-dependent fields must have independently verified semantic types.

The initial no-change recipe needs the original header, raw metadata/type hashes,
object IDs/type indexes, object-table entry bytes, object bytes/gaps/alignment,
external identities, reference-type data and unknown fields retained verbatim.
Texture2D spans alone are insufficient: relocating one object changes the table
and potentially all later objects/data/file bounds. Retain every unaffected
object and unknown region, not just the recognized texture.

For any pixel change:

- Explicitly require a supported ordinary 2D image count/dimension, known mip
  layout, understood color/alpha semantics and safe texture purpose.
- Preserve correct complete-image/storage lengths and all dependent fields, not
  just dimensions and a byte-vector length prefix.
- Sprite/SpriteAtlas, UI, mesh and external consumers need a resolved graph or a
  safe skip. A zero atlas count inside one node is not global protection.
- Stream-backed content requires complete owner/alias maps and a coordinated
  relocation recipe. Do not delete unknown stream gaps or compact a `.resS`
  containing uninspected mesh/pixel consumers.

A bundle recipe additionally retains exact node names/order/flags, decoded node
data/gaps, block flags/codec behavior and all directory offsets/sizes. Content
hashes, load CRCs, CAB names, bundle filenames/cache hashes and catalog/provider
identities are different domains. Byte-preserving recompression rules cannot be
applied to changed decoded content. Do not clear integrity values or generically
re-sign catalogs. The first artifact must stay detached from installed content.

## Other drafts: prerequisites before even a no-change exporter

| Adapter | Required missing contract |
| --- | --- |
| VTF | Exact versioned resource layout, retained unknown resource bodies and flags, thumbnail/image/CRC semantics, surface/mip ordering, material/sheet references |
| VPK | Independent preload + numbered/embedded body reconstruction; collision-free entry identity, retained directory/section layout, CRC/MD5/signature behavior; refuse unsupported signed mutation |
| GameMaker | Authoritative runtime-specific TXTR/TPAG/object schemas, pointer relocation, page/sprite/font geometry, texture groups and embedded codec framing; framing counts are not a rebuild recipe |
| Godot audio | Retained RSRC tables/unknown resource fields and identity, independent codec/frame/loop/timing semantics, no unresolved script/external/subresource consumers; resource rewriting precedes PCK relocation/hash updates |
| IoStore | Trusted companion partition handles, exact stored/codec chunk reconstruction, directory/perfect-hash/hash verification, package-store/Zen/cooked references and integrity; physical offsets alone do not define exportable packages |

## Remaining validation sequence

Initial isolated builds, shared-I/O fixtures, copied-file XNB audits and a
detached XNB export have passed. Continue with format-specific malformed/budget
fixtures → independent inventory/span and pixel comparisons → repeat stability
→ disposable game-copy export checks → runtime visual checks.

Only then consider opt-in apply with recovery and transaction semantics. Existing
installed compaction checkpoints, frozen backend, prior backups and published
release artifacts are not development fixtures and must remain unchanged.
