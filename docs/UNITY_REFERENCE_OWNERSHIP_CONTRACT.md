# Unity reference and stream ownership contract — design only

Date: **2026-10-08**. No PPtr/Sprite/SpriteAtlas consumer reader or ownership
proof is implemented by this document. The current Texture2D draft retains
original spans and external identities only. Storage shape, zero local atlas
counts and in-bounds `.resS` ranges do not authorize resizing.

## Identity must precede graph traversal

- Key an object by **input snapshot identity + exact bundle node index + original
  64-bit path-ID bits**. A path ID alone is not globally unique. A display name,
  GUID, CAB name or filename alone must not merge snapshots/nodes/objects.
- Preserve original node name bytes, node flags, SerializedFile version/endian,
  object type index/class, complete object table span and external-table order.
  Byte equality and namespace lookup are separate evidence.
- A stream is a held validated resource-node/companion identity plus a checked
  half-open range. A normalized basename is not a stream identity. Duplicate node
  names and case/canonicalization collisions require ambiguity statuses.
- Record the exact endian/schema interpretation of `m_FileID` and `m_PathID`;
  signed file-index interpretation and 64-bit path-ID bits must not be confused.
  Admit only schema-proven PPtr fields, never a scan for plausible 12-byte values.

## PPtr resolution results

For the initially targeted SerializedFile v17–22 subset, independently verify
the normal interpretation: file index zero addresses the current SerializedFile,
positive indexes address the ordered **1-based** external table, and path ID zero
is a null reference. Do not promote these conventions into a generic decoder
without exact type-tree, width, signedness and alignment proof.

Each encountered PPtr retains source object identity, indexed field path,
object-relative field spans, original file-index/path-ID bits, declared target
type, and one explicit result:

| Result | Required meaning |
| --- | --- |
| `NULL` | Exact schema-proven null encoding; retain anomalous file-index combinations separately |
| `LOCAL_RESOLVED` | Non-null target matches exactly one object in this node; verify target class where declared |
| `EXTERNAL_UNRESOLVED` | Valid declared index/identity, no proven target mapping; never guessed from a basename |
| `EXTERNAL_RESOLVED` | Independently verified unique mapping in the supplied snapshot set, with exact target object |
| `INVALID_INDEX` / `MISSING_TARGET` | Negative/out-of-range index or absent non-null target under the verified schema |
| `AMBIGUOUS_TARGET` | Duplicate/alias/canonicalization ambiguity; do not pick the first match |
| `UNSUPPORTED_SCHEMA` / `BUDGET_SKIPPED` | Field/object was not interpreted completely; never a missing-edge success |

No external opening is implied. Initially restrict resolution to explicitly
selected same-bundle nodes. Future companion opening requires held trusted roots,
component-wise no-symlink lookup and descriptor checks; it still is not a
filesystem snapshot or sandbox against directory relocation/mount manipulation.

## Consumer evidence needed before changing a texture

1. Start with one independently verified Sprite or SpriteAtlas type-tree schema
   and exact engine version/platform, not a guessed fallback for stripped trees.
   Unknown common strings, managed references and unsupported alignment stay
   opaque. Generic PPtr framing alone does not explain coordinate semantics.
2. Sprite evidence must retain texture/alpha-texture references, original rects,
   offsets, pixels-per-unit, packing/rotation, mesh/UV/vertex/index relations and
   any versioned render-data/atlas indirection. A discovered texture pointer does
   not establish every field that would need coordinated updating.
3. SpriteAtlas evidence must retain packed sprites, texture pages, render-data
   mapping identities, page/rect/rotation and separate alpha-page dependencies.
   Atlas counts or object names are not substitutes for map/key interpretation.
4. Mesh/material/shader/UI/script consumers may depend on logical size, UVs,
   render-target usage or runtime-loaded identities. A graph complete only for
   Sprite objects is not a graph complete for the game. Conservatively mark
   unresolved purposes and runtime/custom consumers as blockers for resizing.
5. Bound recursion/edge/object/text/report work independently. Preserve indexed
   array/map paths, detect cycles with visited identities, and report limits.
   No constructor/script/provider execution, reflection or assembly loading.

The first consumer reader should output evidence and skips only. Do not expose a
writer, automatic container route or installed integration with that increment.

## Stream ownership is a separate completeness proof

Inventory **every possible owner**, including mesh/bulk and unknown class data,
before relocating or deleting any region. Texture2D-only coverage cannot prove
that gaps or intersecting bytes are unused. Retain raw unknown regions exactly.

Distinguish exact declared aliases, intersecting ranges, proven content aliases,
and exclusive ownership. Range union is only arithmetic. Different owners can
share bytes intentionally, and valid bounds do not prove codec or content equality.
Any moved range requires a complete list of schema-proven fields to update across
all referencing objects/files plus recoverable coordinated publication.

Use a coverage ledger for each node/snapshot: total objects by class, inspected
consumer objects, complete vs opaque schemas, found/resolved/unresolved edges,
stream references, unknown owners and budget skips. **Never interpret zero edges
from an uninspected object as zero dependencies.** A closed graph is relative to
the supplied snapshots and schema set, not proof against runtime references.

## Deferred acceptance fixtures

- Both endian interpretations, original field spans and exact PPtr alignment;
  file index 0/1/last/out-of-range/negative, path ID 0 and high-bit IDs.
- Same path ID in different nodes; duplicate node names, GUID/CAB-name collisions,
  unique vs ambiguous external mappings and missing targets.
- Nested arrays/maps with indexed paths, duplicate edges, cycles, unknown common
  strings, malformed later nodes, interruption and aggregate budget exhaustion.
- Sprite/atlas schemas corroborated against an independent parser; coordinates
  retained without inventing resizing transforms.
- `.resS` with overlapping texture and mesh owners, exact aliases, opaque owners
  and nonzero unknown gaps; no region deletion or relocation in reader tests.

Only after independent consumer and completeness validation can an exact no-change
recipe be considered. Unity bundle CRCs, content hashes, CAB names and provider/
catalog/cache identities still require their own contracts. Unknown signed
integrity remains a skip; disabling CRCs or generic re-signing is never a solution.
