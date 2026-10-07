# Follow-up engine/archive candidates

Preliminary source investigation, **2026-10-07**. No new readers, dependencies,
tests, builds, installed scans, decompression or writers were added by this
document. These candidates follow the existing
[reader drafts](ENGINE_DEVELOPMENT_STATUS.md), not replace their validation.
No installed eligible-byte ranking is available.

## Creation/Gamebryo: BA2 before broad BSA claims

BA2 magic `BTDX` has at least two substantially different branches: `GNRL`
general files and `DX10` texture records. The versioned `btdx` 0.3.0 reader
describes Fallout 4 versions 1/7/8 separately from Starfield versions 2/3, with
different header lengths and archive compression selection [B1, B2]. These are
upstream claims and format evidence, not independently validated game support.

The texture branch stores dimensions, mip count, DXGI format and chunk records,
not an ordinary DDS file at each archive offset. Chunk records include physical
offset, stored/decoded size and first/last mip indices. A useful first reader
would inspect an explicit **PC BA2 v1** subset:

1. Signature/version/type, bounded file count, header/record/name-table extents.
   Preserve names as bytes; reject aliases/case ambiguity rather than silently
   normalizing lossy UTF-8 into an identity.
2. Separate GNRL records from DX10 records. Check every body/chunk range against
   source and metadata extents; report stored versus compressed requirements.
3. For DX10, inspect chunk mip coverage/gaps/duplicates and dimensional/format
   arithmetic. Do not equate the `DX10` archive tag with a DDS DX10 header.
4. Initially skip cubes, unknown flags, non-linear tile modes, unsupported DXGI
   values and newer versions. Validate chunk ordering independently before any
   decompression or synthetic DDS reconstruction.

Codec availability is not archive support. Raw LZ4 and zlib framing differ;
bounded decoded size, exact output/consumption and allowed version/codec pairs
need their own gates. Exported DDS can lose archive-only metadata, so an export
decoder does not automatically provide a compatible round-trip repacker.

Later resizing also needs material/NIF consumers, data/normal/specular/color
semantics, every affected chunk/mip boundary, archive lookup hashes, offsets,
name-table identity and mod/load-order behavior. The reviewed library documents
a BA2-specific name hashing scheme [B1]; the project's ordinary CRC32 helper is
not evidence that these hashes can be recreated correctly. No hash algorithm
was implemented by this research.

BSA is a **separate adapter** with its own versions, flags, filename prefixes,
compression conventions and lookup hashes. Do not advertise Skyrim/Oblivion/
Morrowind archive compatibility from a Fallout/Starfield BA2 reader.

## Wwise: useful across engines, not a loose-WAV adapter

Wwise's compiled banks use BKHD header/version metadata, DIDX media indexes,
DATA media/pre-fetch bytes and HIRC behavior/reference objects [W1]. Media can
be embedded, streamed externally or split into a prefetch prefix and streamed
remainder. The same `.wem` can be used by events with different timing or pitch.

The smaller initial deliverable is **versioned bank/media metadata**, not a new
encoder:

- Bounded chunk framing with validated endian/version pairs; duplicate sections,
  malformed lengths, unknown/custom versions and encrypted headers stay skips.
- DIDX ID/range tables checked against DATA, with explicit alias/overlap reporting.
  Never call every DATA extent a complete standalone media file.
- Versioned HIRC object framing and preserved object IDs/type/byte ranges before
  attempting any sound/stream/reference schema.
- Bounded WEM RIFF/RIFX metadata and exact codec identifiers. A WEM extension or
  RIFF signature is not proof of ordinary PCM WAV serialization.

Transcoding requires understood codec/container and seek metadata, preservation
of IDs, loop/frame units, channel layout, timeline trims, durations and cross-bank
references. Music segments can combine multiple stems, transitions and timing
settings. An isolated waveform comparison cannot validate event playback.
Never rename an unsupported codec to Opus or use `.wem` as a filename-only route
into the existing WAV encoder.

The reviewed wwiser documentation explicitly explains cross-bank targets and
event-based Unreal packaging that can place media in cooked `.uasset` resources
[W1]. A loose bank audit will not reach all Unity/Unreal Wwise content; outer
container and package references remain independent requirements. Wwise `.pck`
is unrelated to Godot's `GDPC` format.

No wwiser runtime, extraction script, per-game heuristic, decryption behavior or
implementation was imported. Do not follow its optional workflows for guessing
encrypted versions or treating media as unused: those are outside this project's
preserve-unknown-content contract.

## Defold: archive, compiled resource and manifest form one contract

Defold ships archive index/data/manifest companions (`.arci`, `.arcd`,
`.dmanifest`). Current development documentation describes archive-index v6
packing flags into the high four bits of an offset word and a 60-bit data offset;
resource sizes remain 32-bit [D1]. Do not apply an older 32-bit offset layout to
all versions. Index entries can declare compressed or encrypted resources.

The manifest schema distinguishes **resource-content hashes**, resource URLs and
URL hashes; it also defines dependency lists, engine-version compatibility and a
signature of serialized manifest data [D3]. The archive overview's hash prose
alone is insufficient to choose a lookup identity or verify an index. Inspect
the exact versioned builder/runtime layouts before implementing a reader.

The compiled texture schema includes alternative images, original versus stored
dimensions, depth/count/type, mip offsets/sizes, compressed mip sizes and codec
selection [D2]. It is not simply a `.png` with a different extension. Alternatives
and atlas consumers can be runtime-dependent; dropping them because one GPU can
decode one format is not generally safe.

First deliverable: a version-pinned index/manifest metadata audit with explicit
signature status and companion requirements, then compiled texture/atlas
metadata. Keep encrypted resources, unknown schemas and signed-content mutation
opaque. Retain unknown Protobuf fields if a future rebuilding API is considered.

Do not disable manifest checks or generate a replacement key/signature for an
installed game. A research export that changes a texture can invalidate its
content hash and signed manifest even if the texture bytes independently decode.
This may permanently exclude some shipped configurations from installed asset
writing; filesystem compression/dedupe remain applicable without content edits.

## Priorities and evidence gaps

1. Validate and finish the existing Unity/XNB/Godot/Source reader boundaries when
   authorized; avoid accumulating more apparently supported but unvalidated routes.
2. Investigate PC BA2 v1 metadata as a bounded next texture-archive candidate.
3. Investigate Wwise bank/media addressing as cross-engine audio groundwork.
4. Pin Defold builder/runtime sources and establish signed-manifest constraints
   before implementing an index or texture reader.

No popularity or potential-savings figures were measured. All three families
remain unsupported for dedicated packed asset optimization. Proprietary families
elsewhere in [ROADMAP.md](../ROADMAP.md) need their own investigations; there is
no credible universal-format completion point.

## Sources and provenance

- **B1** [btdx 0.3.0 versioned documentation](https://docs.rs/btdx/0.3.0/btdx/).
- **B2** [btdx 0.3.0 versioned reader source](https://docs.rs/crate/btdx/0.3.0/source/src/read.rs).
- **W1** [wwiser bank/reference documentation](https://github.com/bnnm/wwiser/blob/master/doc/WWISER.md).
- **D1** [Defold archive overview](https://github.com/defold/defold/blob/dev/engine/docs/ARCHIVE_FORMAT.md).
- **D2** [Defold compiled TextureImage schema](https://github.com/defold/defold/blob/dev/engine/graphics/proto/graphics/graphics_ddf.proto).
- **D3** [Defold manifest/dependency/signature schema](https://github.com/defold/defold/blob/dev/engine/resource/proto/resource/liveupdate_ddf.proto).

B1/B2 are version-pinned. W1/D1–D3 were read from upstream branches on the date
above; they are **not immutable references**. GitHub commit lookup timed out,
so no hash was invented. Pin and independently corroborate these sources before
implementing layouts. No upstream code or dependency was vendored; review
licenses separately before any future code reuse (Defold uses its own license).
