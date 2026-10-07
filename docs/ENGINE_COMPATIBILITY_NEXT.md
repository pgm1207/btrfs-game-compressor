# Next engine compatibility steps

Research date: **2026-10-07**. This began as a design investigation. Subsequent
source work added isolated builds, fixtures, copied-file audits and one detached
XNB export. These do not establish tested game support or runtime compatibility.
No installed writes or game launches were performed.

Subsequent local source work added native-only metadata reader drafts, including
VPK directory framing, plus a bounded development-only XNB Texture2D exporter.
There is no installed writer or automatic routing. Their exact scope,
limits and current validation status are recorded in
[ENGINE_DEVELOPMENT_STATUS.md](ENGINE_DEVELOPMENT_STATUS.md). The prerequisites
below remain acceptance targets, not claims that the drafts already satisfy them.

## Recommended sequence

| Order | Target | Next bounded deliverable | Why / limits |
| --- | --- | --- | --- |
| 1 | Unity | Bundle Texture2D **storage/reference inventory**, then a standalone export prototype | Existing bundle reader and sampled type trees provide a foundation; metadata summaries alone do not locate or validate pixels |
| 2 | XNA / MonoGame / FNA content | Uncompressed **XNB v5** reader-table and root Texture2D audit | Smaller format boundary; existing Carrion evidence includes uncompressed v5 files, but their reader IDs/payloads remain unknown |
| 3 | Godot 4 | Versioned binary-resource **AudioStreamWAV** inventory | Extends an existing PCK adapter; codec support alone does not preserve resource identities, loops or timing |
| 4 | Source 1 | Loose PC VTF header/resource/mip audit | Existing BC codecs are reusable later; VTF layout is not DDS layout and packed VPK coverage is separate |
| 5 | GameMaker | FORM/chunk and versioned TXTR/TPAG inventory | Broad potential, but texture pages are atlases with coordinate dependencies, not ordinary images |
| Parallel foundation | Unreal UE4/UE5 | IoStore chunk/block/partition inventory | Pak auditing does not reach UE5 cooked textures; no near-term texture writer implied |
| Deferred | Ren'Py | Restricted RPA index reader | Indexes use serialized Python data; archive parsing must never execute pickle instructions |

This order reflects code reuse and bounded implementation scope, **not a measured
ranking of installed eligible bytes**. No new library census was run. Runtime
engine detection, archive recognition, pixel decoding and compatible rewriting
are four separate claims.

## 1. Unity: prerequisites before a Texture2D writer

### What we actually have

- `native/src/unityfs.rs` decodes supported stored/LZ4/LZ4HC bundle blocks and
  inventories nodes. Its existing lossless export preserves decoded content;
  it is not an arbitrary node replacement/repacking API.
- `native/src/unity_serialized.rs::bundle_summary` reports metadata/class counts.
  Its `texture_bytes` counts **serialized object bytes**, not external pixel
  stream bytes. A tree-present flag is not proof that every Texture2D tree can
  be walked by our restricted inspector.
- `native/src/unity_tree.rs` inspects selected Texture2D fields, but retains
  values in the previously tested path. The local draft additionally retains
  selected original field spans; no lossless object rebuilder is implemented.
- Standalone stream summaries bounds-check declared paths/ranges; they do not
  prove ownership, identify overlapping aliases, or decode the pixels.
- Existing bundle evidence sampled WHAT THE GOLF and Cocoon. It is not universal
  Unity/Addressables coverage. Unity explicitly permits omitting bundle trees
  through `DisableWriteTypeTree` [U1]. Unknown/stripped trees remain a skip.

### First deliverable: storage/reference inventory, no rewriting

1. Retain exact object-table entry spans, type indices, path IDs, object ranges,
   external-reference identities and header widths/endian. The previously tested
   path only counted external references; the local draft retains those identities
   and original Texture2D object/table spans, without resolving PPtrs or rebuilding
   objects. Keep identity separate from offsets and validate span semantics.
2. Inspect supported Texture2D objects inside bundle nodes, using a shared
   bounded API rather than pretending `bundle_summary` already inspects pixels.
   Report inspected, unsupported-schema, malformed and budget-exhausted counts
   separately; do not hide every inspection failure as an undifferentiated opaque
   success.
3. Resolve stream references in the **bundle namespace**, not the host filesystem.
   Unity's `archive:/...` identities and `PPtr` external tables identify mounted
   SerializedFiles [U2]; stream resolution needs its own validated rules. Preserve
   node names byte-for-byte; reject ambiguous/duplicate identities rather than
   matching only a basename or assuming every `.resS` belongs to one texture.
4. Build a range/owner map for texture streams. Exact aliases, partial overlaps,
   unknown consumers and references leaving the supplied input set are blockers
   for compaction. `.resS` can contain **mesh as well as texture bytes** [U3].
   Never delete unknown/unreferenced-looking stream regions to create savings.
5. Inspect Sprite/SpriteAtlas references and record unresolved external
   dependencies. An atlas count of zero in one file is not proof that no sprite
   elsewhere references its texture. Skip unresolved/atlas/UI/data/normal/HDR
   candidates; texture-name hints alone are not sufficient proof of purpose.
6. Check per-codec mip byte totals against the exact inline/streamed extent.
   Start with ordinary 2D DXT1/DXT5/RGBA32 only. Crunch, arrays, cubes, virtual
   textures, platform-swizzled data and unknown payload layouts remain opaque.

**Path safety prerequisite:** the previously tested standalone resolver used
`symlink_metadata(parent.join(relative))`, rejecting a final symlink but not
intermediate-directory symlinks. The local, unvalidated source now uses
component-by-component `openat` with `O_NOFOLLOW` (`O_DIRECTORY` for directories).
Before reading stream contents, validate this helper and establish a trusted
root descriptor and opened-source/companion identity/change contract. The
initial supplied root, directory relocation and mount behavior are separate containment concerns;
canonicalize-then-open is not race-resistant containment.

### Second deliverable: standalone export prototype

- Support one fully understood little-endian SerializedFile version/layout
  initially, not every v17–22 layout merely because the audit accepts its header.
- Add field spans for dimensions, mip count, payload lengths, complete image
  size, stream offset/size and all other size-dependent fields in that exact
  schema. Preserve unknown fields, type hashes and object IDs verbatim.
- Prove a no-change object/node rebuild before changing pixels. A smaller object
  requires new object offsets/sizes and file/data/header sizes, with correct
  alignment. Do not apply a fixed byte-offset recipe based on Unity version names.
- Start with self-contained inline storage; stream-backed exports require a
  complete ownership map and coordinated relocation. Preserve unrelated bytes
  and dependencies. If retaining stream holes, report that pixel reduction may
  not reduce the stream/container file length.
- Prefer retaining a valid existing lower-mip suffix when suitable. This avoids
  additional BC re-encoding error, but still changes texture dimensions and
  requires matching metadata/reference checks. It is **not** byte-preserving
  asset optimization or permission to resize an atlas.
- For re-encoding, factor a bounded surface/codec interface out of the DDS
  machinery instead of treating a DDS header as a Unity texture payload. Color
  space, premultiplied/straight alpha and semantic texture roles must survive.
- An archive rebuild must retain node flags/names/ordering, relocate every changed
  node, rebuild decoded blocks and metadata, and handle the uncompressed-content
  hash consistently. Existing lossless recompression preserves its old hash
  precisely because decoded content does not change; a lossy writer cannot.

### Integrity is a separate gate

Unity's load CRC is over **uncompressed content**, not simply compressed file
bytes [U4]. Byte-preserving recompression and texture rewriting therefore have
different CRC consequences. Addressables can verify CRC, use bundle hashes for
names/cache identity, and use custom providers [U5]. CAB/internal names, API
hashes, filenames, catalog hashes and CRCs are not interchangeable [U2].

The first prototype must export a research artifact, not a drop-in replacement.
Do not clear CRC fields, rename CAB nodes, disable checks, or claim catalog
compatibility. Installed apply stays disabled until exact catalog/provider and
dependency behavior is supported. External signed/custom integrity checks remain
blockers; there is no generic re-signing solution.

### Resource/UX work before exposing the richer audit

The existing inventory can hold up to 2 GiB of decoded bundle data plus source and
temporary block buffers. **Read-only does not mean lightweight.** A future
user-facing route needs explicit per-file/aggregate decoded-work and peak-memory
budgets, cancellation, and block-range decoding for selected nodes where feasible.
Escape untrusted node names/control characters for pipe-delimited output.

Prefer a dedicated inventory option rather than silently replacing or appending
to `--audit-container`'s existing output: that command currently runs the UnityFS
recompression analysis, including CPU-intensive compression in memory. Preserve
the existing output contract and make work cost visible.

## 2. XNB: a smaller independent compatibility increment

Upstream MonoGame reads a header, optional decompressed size, a reader
table, a shared-resource count, and a root reader index [X1–X3]. Texture2D data
then contains surface format, width, height, mip count and a length-prefixed blob
per mip [X4]. Do **not** instantiate assemblies or reflection types named by an
untrusted file; record reader names and use an explicit static allowlist.

### Initial read-only subset

- Uncompressed XNB **v5**, conventional little-endian desktop targets whose
  payload layouts are independently established. Existing header recognition of
  v4–6 does not establish support for their object serialization.
- Checked .NET-style 7-bit lengths/indices, strict UTF-8 reader strings, signed
  reader versions, bounded counts and a capped metadata/payload read budget.
- Known root `Microsoft.Xna.Framework.Content.Texture2DReader`, supported reader
  version, no shared resources; preserve assembly qualification in reports and
  validate any allowed normalization explicitly. Unknown/custom readers are opaque.
- Report surface IDs, dimensions and exact mip ranges. For v5 candidates, audit
  Color (0), Dxt1 (4), Dxt3 (5), Dxt5 (6) first [X5]. Verify byte order/alpha before
  introducing a writer; comments saying ARGB/RGBA are not enough evidence.
- Validate each mip against checked dimensions/block lengths, then require full
  root consumption for this subset. Surface IDs are **not** Unity TextureFormat
  IDs or DXGI IDs. LZX, LZ4, v4 legacy formats, v6 and unknown IDs stay opaque.

**Important version trap:** FNA maps XNB v4 legacy format values 1/28/30/32 to
BGRA/DXT1/DXT3/DXT5; v5 uses the modern enum [X6]. Reusing a v5 format map for v4
can silently misinterpret payloads. MonoGame's LZ4 reader is a token-stream
decoder [X7], not evidence of an LZ4 frame; confirm framing/exact consumption
before reusing our existing LZ4 library.

A detached development writer now retains the original reader table and identity,
checks a byte-identical no-change rebuild, and exports a resized single BC mip
to a new path. Skip SpriteFont roots, whose glyph/cropping rectangles
refer to an embedded texture [X8]. Even a standalone Texture2D can be an atlas
referenced by external scripts: decoding does not establish safe resizing.

Existing evidence: [Carrion header trial](../test/engine-results/carrion-steam-trial-2026-10-03.md).
Its comic/art-book headers make v5 a plausible starting point, not proof of
reader type, image purpose, compatibility, or potential savings.

## 3. Godot 4 audio: extend the resource layer, not just the encoder

Godot 4.3 `AudioStreamWAV` stores `data`, `format`, `loop_mode`, `loop_begin`,
`loop_end`, `mix_rate` and `stereo`; its formats include 8/16-bit PCM, IMA ADPCM
and QOA [G1]. These packed resources are not RIFF WAV files. The existing loose
WAV writer cannot simply be pointed at a PCK resource.

Next: versioned RSRC/RSCC binary-resource inventory with string/property tables,
subresource identities, references and variant decoding. Report codecs, channels,
rates, frame counts and loops; unknown resource compression/version remains a
skip. Start a future export with non-looping PCM only, preserving resource type,
UID/path identity and channel layout. Rate changes need duration/frame checks;
looping resources need exact loop-unit/rounding semantics before enabling them.

`AudioStreamOggVorbis` references an `OggPacketSequence` and exposes loop offset,
BPM, beat count and bar beats [G2]. It is not necessarily a standalone `.ogg`
byte stream. Vorbis transcode needs packet/granule/seek data and timing metadata
rebuilt together. QOA/ADPCM and rhythm/loop-bearing resources should initially
remain untouched. Reuse the PCK directory/hash writer only after the resource
payload and reference contract is independently understood.

## 4. Source 1 VTF: codec reuse, different layout

Valve's SDK documents PC VTF 7.x headers and resource entries [S1]. Image data
on disk is ordered **smallest mip first**, then animation frames and faces;
our DDS path is not a drop-in VTF parser. Resource entries can hold inline data
instead of offsets, and legacy image resources omit the ordinary length tag.
The SDK also warns that C++ struct sizes/padding do not directly specify disk
offsets. Parse explicit versioned fields, not a native struct cast.

First audit: conservative known PC 7.1–7.4 layouts, dimensions, depth, frame/face
counts, mip ranges, low-resolution thumbnail, resources and flags. Normal/SSBump,
procedural, cube, volume, animated and sheet/atlas textures are writer blockers.
Later export can consider ordinary 2D DXT1/DXT3/DXT5 with original color/alpha
flags and unchanged unknown resources. VMT/material dependencies and image-purpose
constraints remain separate. VPK preload segments, archive offsets/checksums and
any signatures need their own reader/repacker; no Source 2 VTEX_C claim follows.

Valve's developer wiki returned an anti-bot page during this investigation; these
findings use SDK source instead. Review upstream license terms before copying
code; **no SDK implementation or library is vendored by this research**.

## 5. GameMaker: atlas geometry and runtime versions first

UndertaleModTool's reader shows multiple TXTR record variants: generated mip
fields, 2022.3+ block sizes, and 2022.9+ dimensions/group indexes and external
texture handling [M1]. Its TPAG model distinguishes source rectangles on the
page from target/bounding rectangles used for logical rendering [M2].

Next: bounded FORM chunk directory plus known-version TXTR/TPAG inventory; no
PNG/QOI signature hunting followed by replacement. Validate pointers, chunk
lengths, image extents, external texture groups and sprite/font/page ownership.
PNG, GameMaker QOI variants and BZip2-wrapped QOI are distinct layouts, not proof
that the generic `.qoi` decoder/writer is compatible.

First potential writer should preserve page dimensions/geometry (lossless
same-format improvement, if measurable) before attempting downscaling. A future
resizer must update page source rectangles while preserving logical target size,
origins, collision geometry and font metrics. Tiny entries/gutters/rounding and
game code that queries texture sizes can still prevent safe resizing. No writer
is justified by a `data.win` filename alone. Upstream code/dependency licenses
must be reviewed before reuse; this research copies no implementation.

## 6. Unreal: finish addressing before cooked textures

The reviewed IoStore implementation separates chunk IDs and logical offset/length
from physical compressed-block offsets and partition selection [E1]. Next:
parse versioned chunk/offset tables, method names, compression blocks, perfect-hash
tables and directory-index bounds. Report decoded codec requirements and resolve
physical ranges against validated companion partitions without decoding them.
Header counts/companion existence do not prove chunk validity.

Only then attempt bounded stored/permitted-codec chunk exports, package-store/Zen
references, cooked schemas and texture bulk payloads. Signed/encrypted layouts,
unknown custom versions and unavailable proprietary codecs stay unchanged. Do not
copy upstream per-game heuristics into a supposedly universal adapter.

## 7. Ren'Py: an archive reader must not execute code

Ren'Py's RPA v2/v3 readers decompress an index and deserialize Python data; v3
also deobfuscates offsets/lengths and can include prefix bytes/segments [R1]. A
compressor must not use unrestricted `pickle.loads`, execute `GLOBAL`/`REDUCE`,
or import objects from an archive. Design a restricted primitive-data decoder
with opcode, nesting, item-count and decompressed-byte limits, or leave the index
opaque. Preserve segment reconstruction and safe paths. Image downscaling also
needs logical-size/script/UI protection; RPA recognition alone does not solve it.

## Remaining acceptance work

For each new subset: malformed/version/budget fixtures; independent parser and
unchanged-resource comparison; export-only source preservation and destination
refusal; exact mip/reference/loop checks; repeat stability; then manually
authorized game-copy visual/listening checks. A parser/compiler pass is not a
playtest. No installed apply, release tag or support promotion before these gates.

The subsequent source work introduced only a detached XNB exporter behind the
development feature. It is not exposed by the Bash UI or installed asset pipeline.
No support level was promoted.
Keep research local while format-specific validation is incomplete; normal
pushes trigger repository CI, including builds and tests.

## Sources (accessed 2026-10-07)

Source links are pinned to the reviewed repository snapshots where available;
documentation URLs are versioned where practical. They are format evidence,
not independent validation of our parser or of any shipped game's compatibility.

- **U1** [Unity DisableWriteTypeTree](https://docs.unity3d.com/ScriptReference/BuildAssetBundleOptions.DisableWriteTypeTree.html).
- **U2** [UnityDataTools AssetBundle format](https://github.com/Unity-Technologies/UnityDataTools/blob/b73e33c1847ff81d43bc1da3726145a9a4c97c3a/Documentation/assetbundle-format.md).
- **U3** [UnityDataTools Player build format](https://github.com/Unity-Technologies/UnityDataTools/blob/b73e33c1847ff81d43bc1da3726145a9a4c97c3a/Documentation/playerbuild-format.md).
- **U4** [Unity AssetBundle.LoadFromFile CRC](https://docs.unity3d.com/ScriptReference/AssetBundle.LoadFromFile.html).
- **U5** [Addressables 1.21 Content Packing & Loading schema](https://docs.unity3d.com/Packages/com.unity.addressables@1.21/manual/ContentPackingAndLoadingSchema.html).
- **U6** [UnityDataTools archive-node flags](https://github.com/Unity-Technologies/UnityDataTools/blob/b73e33c1847ff81d43bc1da3726145a9a4c97c3a/UnityFileSystem/DllWrapper.cs).
- **X1** [MonoGame ContentManager](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Content/ContentManager.cs).
- **X2** [MonoGame ContentTypeReaderManager](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Content/ContentTypeReaderManager.cs).
- **X3** [MonoGame ContentReader](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Content/ContentReader.cs).
- **X4** [MonoGame Texture2DReader](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Content/ContentReaders/Texture2DReader.cs).
- **X5** [MonoGame SurfaceFormat](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Graphics/SurfaceFormat.cs).
- **X6** [FNA Texture2DReader (legacy v4 mapping)](https://github.com/FNA-XNA/FNA/blob/08f668a9ded6362102b77ef8e2d754e88fc62481/src/Content/ContentReaders/Texture2DReader.cs).
- **X7** [MonoGame Lz4DecoderStream](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Utilities/Lz4Stream/Lz4DecoderStream.cs).
- **X8** [MonoGame SpriteFontReader](https://github.com/MonoGame/MonoGame/blob/55b5621bcea08cb5fd8d7769ecb1d27de2e4248f/MonoGame.Framework/Content/ContentReaders/SpriteFontReader.cs).
- **G1** [Godot 4.3 AudioStreamWAV](https://github.com/godotengine/godot/blob/24f8d6ecc79d69e0272dc3ff8da53d5bfabc970c/scene/resources/audio_stream_wav.cpp).
- **G2** [Godot 4.3 AudioStreamOggVorbis](https://github.com/godotengine/godot/blob/24f8d6ecc79d69e0272dc3ff8da53d5bfabc970c/modules/vorbis/audio_stream_ogg_vorbis.cpp).
- **G3** [Godot 4.3 resource binary layout/variant handling](https://github.com/godotengine/godot/blob/24f8d6ecc79d69e0272dc3ff8da53d5bfabc970c/core/io/resource_format_binary.cpp).
- **G4** [Godot 4.3 resource binary flags/reserved fields](https://github.com/godotengine/godot/blob/24f8d6ecc79d69e0272dc3ff8da53d5bfabc970c/core/io/resource_format_binary.h).
- **S1** [Valve Source SDK VTF headers/layout/flags](https://github.com/ValveSoftware/source-sdk-2013/blob/b8cfb12c0e083a2ef5b2f9f9b50f3902fa034474/src/public/vtf/vtf.h).
- **S2** [Valve Source SDK image-format enum](https://github.com/ValveSoftware/source-sdk-2013/blob/b8cfb12c0e083a2ef5b2f9f9b50f3902fa034474/src/public/bitmap/imageformat.h).
- **S3** [ValvePak VPK directory/section reader](https://github.com/ValveResourceFormat/ValvePak/blob/cfe3fe90b0ea69818b70845feebaea35a8ecac70/ValvePak/ValvePak/Package.Read.cs).
- **S4** [ValvePak VPK entry framing](https://github.com/ValveResourceFormat/ValvePak/blob/cfe3fe90b0ea69818b70845feebaea35a8ecac70/ValvePak/ValvePak/PackageEntry.cs).
- **M1** [UndertaleModTool embedded textures](https://github.com/UnderminersTeam/UndertaleModTool/blob/f43e12c445c37d50dc6244caa12ccab232983f3f/UndertaleModLib/Models/UndertaleEmbeddedTexture.cs).
- **M2** [UndertaleModTool texture page geometry](https://github.com/UnderminersTeam/UndertaleModTool/blob/f43e12c445c37d50dc6244caa12ccab232983f3f/UndertaleModLib/Models/UndertaleTexturePageItem.cs).
- **M3** [UndertaleModTool chunk/padding/table framing](https://github.com/UnderminersTeam/UndertaleModTool/blob/f43e12c445c37d50dc6244caa12ccab232983f3f/UndertaleModLib/UndertaleChunkTypes.cs).
- **M4** [UndertaleModTool version-specific chunk readers](https://github.com/UnderminersTeam/UndertaleModTool/blob/f43e12c445c37d50dc6244caa12ccab232983f3f/UndertaleModLib/UndertaleChunks.cs).
- **E1** [CUE4Parse IoStoreReader](https://github.com/FabianFG/CUE4Parse/blob/2c4dca1d466b59aa36dd7ba235d10478a78074cc/CUE4Parse/UE4/IO/IoStoreReader.cs).
- **E2** [CUE4Parse IoStore TOC section ordering](https://github.com/FabianFG/CUE4Parse/blob/2c4dca1d466b59aa36dd7ba235d10478a78074cc/CUE4Parse/UE4/IO/Objects/FIoStoreTocResource.cs).
- **E3** [CUE4Parse IoStore block bit fields](https://github.com/FabianFG/CUE4Parse/blob/2c4dca1d466b59aa36dd7ba235d10478a78074cc/CUE4Parse/UE4/IO/Objects/FIoStoreTocCompressedBlockEntry.cs).
- **E4** [CUE4Parse IoStore chunk offset/length encoding](https://github.com/FabianFG/CUE4Parse/blob/2c4dca1d466b59aa36dd7ba235d10478a78074cc/CUE4Parse/UE4/IO/Objects/FIoOffsetAndLength.cs).
- **E5** [CUE4Parse IoStore versioned chunk-metadata stride](https://github.com/FabianFG/CUE4Parse/blob/2c4dca1d466b59aa36dd7ba235d10478a78074cc/CUE4Parse/UE4/IO/Objects/FIoStoreTocEntryMeta.cs).
- **R1** [Ren'Py archive handlers](https://github.com/renpy/renpy/blob/68fdaef917919e330c90765dd69668fee1cb9654/renpy/loader.py).
