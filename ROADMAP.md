# Engine support and roadmap 

Status: **2026-10-07**, release **0.2.1**; **0.3.0 unreleased** adds the texture
compression MVP and the read-only UnityFS bundle inventory. Engine asset writers
remain beta and are expanded engine by engine; see [SUPPORT.md](SUPPORT.md) for
what is tested.
This is a quick market/format survey, not an exhaustive catalogue of every engine,
fork, middleware or private studio tool. Scope is shipped PC/Steam game content;
mobile/web-first tools are included where their formats could reach a PC build.

The [next compatibility design](docs/ENGINE_COMPATIBILITY_NEXT.md) records the
2026-10-07 source investigation and concrete Unity, XNB, Godot audio, Source VTF,
GameMaker, IoStore and Ren'Py prerequisites. It is research, not additional
validated support; initial local build and shared-I/O fixture checks are recorded
in the development status, with format-specific validation still pending.
Subsequent native-only reader drafts and their limits are tracked in
[ENGINE_DEVELOPMENT_STATUS.md](docs/ENGINE_DEVELOPMENT_STATUS.md). Unchecked
milestones below are still pending even where draft code now exists.
Preliminary [BA2/Wwise/Defold follow-up research](docs/ENGINE_FOLLOWUP_CANDIDATES.md)
records additional archive, reference and integrity requirements, not new support.

## What “support” means

1. **Filesystem:** Native Zstd and byte-verified Btrfs dedupe are engine-independent.
   Every engine below can use this layer on eligible Btrfs files. Savings are not
   guaranteed; already-compressed/encrypted media may yield little or nothing.
2. **Detection/audit:** identify an engine, container or version without rewriting.
   Detection is evidence, not certification. Extensions alone are insufficient.
3. **Export:** rebuild supported content into a new file; never imply live apply.
4. **Asset apply (beta):** opt-in lossy/savings-gated transforms in the main pipeline.
   Native/Lossless never degrade assets. Unknown layouts remain unchanged.

**G = generic loose formats only:** supported raster images, bounded legacy and
DX10 DDS subsets listed in [SUPPORT.md](SUPPORT.md), and simple mono/stereo
PCM/float WAV, with shared texture/audio policy and backups. Supported standalone
FMOD FSB5 Vorbis banks are also available
regardless of engine. G does **not** mean an engine's packed assets are supported.
Loose resizing cannot always preserve implicit pixel-coordinate assumptions in
scripts; even G needs manual visual testing. Archives are not recursively treated
as ordinary images. No proprietary encoder, external media command or runtime
service is required or planned.

## Engine support chart

All rows inherit the filesystem layer. “None” below means no dedicated engine
adapter, not that Native compression is unavailable. Planned items are not promises
of compatible rewriting across an entire engine family.

| Engine / shipped version family | Current detection / audit | Texture asset rewriting | Audio / lossless container rewriting | Missing work / priority |
| --- | --- | --- | --- | --- |
| **Godot 3.x** | GDPC signature; PCK v1 directory/resources | Beta apply/export for supported GDST `.stex`; atlas/small/thin guards | PCK MP3-resource → Vorbis, supported PCM; PCK dedup export | More import/resource variants and reference-aware protection; P1 |
| **Godot 4.x** | Plain PCK v2–4 audit; GST2 inventory | Beta apply/export for supported GST2 in standalone PCK v3/v4; reliable logical size required | Packed audio not implemented; PCK dedup export | AudioStreamWAV/Vorbis packet resources, QOA, embedded packs, unsupported texture codecs; P1 |
| **Unity legacy / 2017–2022 LTS / Unity 6** | Player/layout markers; UnityFS audit and read-only bundle node/SerializedFile inventory; standalone SerializedFile v17–22 metadata and supported type-tree Texture2D fields | G only; **no SerializedFile Texture2D writer** | UnityFS v6–8 same-codec LZ4/HC **export only**; embedded AudioClip streams not rewritten | Bundle node/block rebuild with `.resS` relocation and object-table shifts, SpriteAtlas/UI protection, catalog/CRC integrity; **P0** |
| **Unreal 4.x / 5.x** | Pak footer v1–11, bounded primary SHA1; v1–9 directory/data-header/stored-payload checks; v10/v11 secondary SHA1 + entry classification; IoStore TOC v1–8 header/bounds/security inventory | G only; **no cooked Texture2D writer** | No Pak/IoStore repacker or packed SoundWave transcode | IoStore chunk/block and package-store/Zen references first for UE5 textures; compressed entry decode; cooked schemas; **P0** |
| Unreal 1–3 / licensed forks | No dedicated adapter | G only | None | Legacy package/compression formats; do not reuse UE4 parser; P3 |
| **GameMaker / Studio / modern runtime** | `data.win` heuristic | G only; packed texture pages untouched | None | FORM chunk reader/writer, texture-page/sprite/font geometry and audio references; P2 |
| **RPG Maker XP/VX/Ace; MV/MZ** | None | G only; no packed/encrypted asset adapter | None | Distinguish RGSS archives from MV/MZ NW.js output; tileset/animation coordinate protection; encrypted assets remain skipped; P2 |
| **Ren'Py 6–8 / Pygame** | None | G only | None | RPA versioned indexes/segments; preserve script/save IDs and logical image sizes; avoid unsafe pickle evaluation; P2 |
| **Source 1 / GoldSrc** | None | G only; VTF/WAD untouched | None | VPK/WAD archives, VTF mip/face/format parsing and material references; P2 |
| **Source 2** | None | G only | None | VPK plus compiled resource blocks, VTEX_C texture/bulk layouts and sound metadata; separate from Source 1; P2 |
| **XNA / MonoGame / FNA** | XNB signature/header inventory; no engine detector | G only; XNB payloads untouched | None | XNB reader IDs/payload compression, texture surface/mips and SoundEffect data; P2 |
| **Hades custom pipeline** | PKG/Bink/FMOD inventory; not a general engine detector | G only; XNB/atlas payloads unchanged | Hades v7 LZ4 PKG lossless apply; supported standalone FMOD beta apply/export | Texture/atlas-aware XNB rewrite; embedded audio still untouched; P1 |
| **Frostbite** | None | G only | None | CAS/CAT/bundle layouts, content hashes, texture streaming and platform versions; P3 |
| **RE Engine** | `re_chunk_*` markers; `.pak` inventory, not Unreal parsing | G only; TEX untouched | None | RE archive indexes, TEX mip/streamed companions, per-format versions; P3 |
| **RAGE** | None | G only | None | RPF version/encryption boundaries, YTD textures, dependent resources; P3 |
| **Creation / Gamebryo** | None | G only; no archive DDS adapter | None | BSA/BA2 records and texture chunks, mip metadata, material references; P2 |
| **id Tech 1–7 / descendants** | None | G only | None | WAD/PAK/PK3/resource archives differ by generation; virtual-texture/page-table handling for newer versions; P3 |
| **CryEngine / Lumberyard / O3DE** | None | G only | None | Versioned package/catalog readers, split DDS mip streams, asset IDs/dependencies; related ancestry is not shared-format proof; P3 |
| **Anvil / Dunia / Snowdrop / Disrupt** | None | G only | None | Separate archive and texture adapters for each family; hashes/streamed dependencies; P3 |
| **Decima** | None | G only | None | Versioned content archives, streamed texture data and integrity checks; P3 |
| **REDengine** | None | G only | None | Different archive generations, cooked resource schemas and texture/streaming metadata; P3 |
| **IW / Treyarch / Sledgehammer variants** | None | G only | None | Fastfile/package variations, streamed texture/audio indexes; online anti-cheat is a major limit; P3 |
| **Northlight / Glacier / 4A / Apex** | None | G only | None | Independent family-specific package/texture/stream adapters, not one “custom engine” parser; P3 |
| **Insomniac proprietary engines / Foundation / Luminous / Ryu Ga Gotoku** | None | G only | None | Version/platform-specific archives, tiled/swizzled GPU payloads and reference graphs; P3 |
| **Defold** | None | G only | None | Manifest/archive hashes and compiled texture/atlas formats; P2 |
| **Cocos2d-x / Cocos Creator** | None | G only | None | Identify native vs web bundles, atlas descriptors, PVR/ETC/ASTC and hashed manifests; P3 |
| **Construct / GDevelop / GameSalad / Stencyl** | None | G only | None | Separate desktop/web wrappers, atlas/layout metadata and resource manifests; P3 |
| **LÖVE / libGDX / SDL / SFML / raylib / custom lightweight engines** | None | G only | None | ZIP/JAR/custom archives where present; no universal engine-specific layout; P3 |
| **Adventure Game Studio / Visionaire / Wintermute** | None | G only | None | Versioned adventure-game resource packs, palette/sprite and audio references; P3 |
| **KiriKiri / NScripter / Adobe AIR / legacy Flash** | None | G only | None | XP3/NSA/SWF/AIR layouts, script image dimensions and codec declarations; P3 |
| **Stride / Flax / Unigine / Torque / Panda3D / Ogre-based games** | None | G only | None | Independent cooked content/package adapters and texture/bulk references; P3 |
| **Custom engines and unknown forks** | Unknown/fallback inventory | G only | None | Signature + format-version evidence; never infer a writer from a generic `.pak`/`.tex` suffix; ongoing |

Engine versions, archive versions and texture codec versions are separate axes.
For example Unity 6 is not UnityFS version 6, and Unreal Engine 5 is not Pak
version 5. Support claims must name the exact tested combination, platform,
codec, container layout and operation (audit/export/apply).

## Shared codecs and middleware: separate from engine support

| Format / middleware | Current | Next work |
| --- | --- | --- |
| Loose raster / DDS | Shared guarded beta apply; in-place DDS and export-only `--texture-compress` use legacy BC1/2/3, 32-bit BGRA/RGBA, DX10 BC1/2/3/4/5/7 and R8G8B8A8/B8G8R8A8 with mip rebuilding | Independent format verification and runtime coverage; arrays/cubes/HDR/normal maps require explicit handling |
| Godot BC1/2/3/7, PNG/WebP/raw | Supported subsets with original/logical size and mip rebuilding | Basis/ETC/ASTC/half-float and compressed metadata remain unsupported |
| PCM / float WAV | Supported simple mono/stereo files, profile ceilings | Metadata-rich/looped/multichannel WAV must preserve timing/chunks before enabling |
| FMOD FSB5 Vorbis / RIFF FEV banks | Bounded standalone beta apply/export; codebook, savings and waveform gates | Broader seek/playback fixtures, embedded stream references, other codecs; manual listening |
| Wwise WEM/BNK/PCK | Inventory only | Native codec + bank/index/loop rebuilding; never rename to Opus |
| CRI ADX/HCA/AWB/ACB | AWB inventory only | Codec feasibility/licensing, cue and stream table references |
| Loose Ogg/MP3/FLAC/Opus/AAC | Inventory; no generic lossy apply | Preserve codec/container, comments/loops/granules, duration and channel layout; same profile policy |
| Bink 1/2; other game video | Inventory only | Compatible encoder under no-proprietary-tools constraint; otherwise permanently skip |
| ZIP/LZ4/Zlib/Zstd and proprietary codecs | LZ4 in bounded known adapters, Zstd at filesystem layer | Same-codec lossless passes only after container verification; Oodle-dependent layouts stay skipped without a permitted compiled-in solution |

## Delivery roadmap

Priority: **P0** next engine foundations; **P1** existing adapters; **P2** subsequent
high-value families; **P3** deferred investigations, not scheduled commitments.

### M0 — Compatibility-first baseline (0.2.0 development)

- [x] Main-pipeline Godot transforms, shared conservative texture guards.
- [x] Shared profile-based audio + standalone FMOD apply/restore.
- [x] Byte-preserving Mech Havoc trial, texture verifiers and real-Btrfs tests.
- [x] Publish support chart and clear unreleased/release distinction.
- [ ] Complete manual visual/listening coverage before claiming runtime certification.
- [ ] Validate both release architectures and release workflow before tagging.

### M1 — Unity foundations (candidate 0.3.0, no release date)

- [x] Bounded standalone SerializedFile v17–22 audit: header/file
  version/endian/platform/Unity version, type-tree availability, types, objects
  and external-reference count. Metadata limit 32 MiB; object payloads are opaque.
- [x] Report Texture2D/AudioClip/Sprite/SpriteAtlas class counts without guessing
  their payloads. Reject duplicate IDs, overlaps and out-of-bounds objects.
- [ ] Versioned type-tree reader; explicit validated schema fallback for stripped
  player builds. Unknown or stripped versions never trigger trial-and-error writes.
  Note: read-only `unityfs-inventory` on real Addressables bundles found type trees
  **present** (e.g. 92 Texture2D objects in one Cocoon bundle), so bundle content
  with supported trees can proceed without a stripped-schema fallback. This is
  sample evidence, not universal coverage: `DisableWriteTypeTree` can strip bundle
  schemas too. Metadata tree presence does not prove field-walker compatibility.
  Storage/reference resolution and container rebuild remain prerequisites. See
  `test/engine-results/unity-bundle-inventory-2026-10-07.md`.
- [x] Bounded type-tree-guided Texture2D field inspection for supported node
   layouts: dimensions, numeric format ID, mip count, inline bytes and declared
   stream path/offset/size. Unknown trees remain opaque. No stream payload is read;
   separate path metadata checks only establish declared extent bounds.
- [ ] Texture2D codec/dimensions/mips/stream-offset inventory, including atlases
  and shared streams. Build fixtures before enabling a writer.
  (Dimensions/format/mips/inline-vs-streamed extents, shared-stream grouping and
  same-directory bounds-checked stream resolution are inspected read-only, with
  per-format byte totals and a file-level atlas/UI risk flag;
  full SpriteAtlas reference resolution and any writer remain pending.)
- [ ] Bundle-namespace stream ownership/alias inventory, retained external PPtr
  references and field/object-table spans. Harden standalone stream containment
  against intermediate symlinks before reading payloads; see the next design.
- [ ] Export-first inline BC1/BC3/RGBA subset, complete mip chains and metadata;
  preserve alpha/color space and skip normal/data textures unless understood.
- [ ] `.resS` relocation and all referencing objects updated together. Multi-file
  apply needs a recoverable transaction/journal, not two unrelated renames.
- [ ] Bundle block/node rebuild plus CRC/hash/catalog consistency. Addressables
  content checks are distinct from per-block decompression verification.
- [ ] AudioClip references to `.resource` must be rebuilt before any embedded
  FMOD transcode is allowed. Preserve sample/frame/loop/stream flags.
- [ ] Independent decode/reference validation, repeat-apply test, copy playtests;
  only then main-pipeline apply. No per-game sizing recipes.

### M2 — Unreal foundations (candidate 0.4.0, independent workstream)

- [x] Separate read-only IoStore `.utoc` header inventory (TOC v1–8),
  version-aware minimum extents, security flags and metadata-only companion
  presence. Real v6/v8 headers surveyed; no chunk/directory/payload verification.

- [x] Bounded primary-index SHA1 check for unencrypted Paks; explicitly skip
  encrypted or >256 MiB indexes. Report `.sig` presence and frozen index flag.
  This is corruption checking, not signature verification or entry validation.
- [ ] Validate bounded unencrypted Pak indexes/entries and their hashes; report
  signature/encryption/codec blockers, not merely footer declarations.
- [x] Unencrypted v1–9 directory/data-header consistency and eligible stored-payload
  checks; v10/v11 primary + PHI/FDI secondary-index SHA1 verification and bounded
  encoded-entry classification (method, encryption, sizes, entry kinds). Unparseable
  but hash-matching bodies are reported as UNPARSED rather than treated as corruption.
- [x] Confirmed on real UE5 IoStore paks that shipped `.pak` files contain
  config/Wwise/raw media, not cooked `.uasset`/`.uexp`/`.ubulk`: cooked textures
  live in `.ucas`/`.utoc`. IoStore is therefore a first-class prerequisite, not an
  add-on, for Unreal texture work.
- [ ] Same-codec lossless export + independent byte comparison before any cooked
  asset changes. Do not claim IoStore support from Pak support.
- [ ] Cooked package summary/name/import/export tables; exact engine/custom
  versions and tagged/unversioned properties. Reject unknown schemas.
- [ ] Texture2D platform data/mip chains and inline/`.uexp`/`.ubulk` streaming
  references. Initially skip virtual textures, cubes/arrays, normals/HDR.
- [ ] Export-first BC1/BC3/RGBA subset with preserved codec, color/alpha semantics,
  LOD/streaming metadata, all offsets and package hashes.
- [ ] Separate IoStore `.utoc`/`.ucas` TOC, chunk/block IDs, package store/Zen data
  and integrity support; signed/encrypted layouts stay unchanged.
- [ ] SoundWave and middleware references retain declared codec, timing, loops,
  cues and streaming chunks; no unconditional PCM→Opus replacement.
- [ ] Journaled multi-file install, independent validation and manual copy tests
  before beta main-pipeline apply. Anti-cheat is not auto-detected.

### M3 — Broaden proven formats (later 0.x minors)

- [ ] Godot 4 audio and missing Godot formats; broaden FMOD playback fixtures.
- [x] Bounded uncompressed XNB v5 reader-table/root Texture2D audit and a detached
  development export for single-mip BC textures; v4 surface IDs, compressed
  payloads, SpriteFont/custom readers and runtime validation remain separate
  gates. No arbitrary reader assembly execution.
- [ ] Versioned Godot 4 AudioStreamWAV resource/loop inventory and loose PC VTF
  resource/mip inventory. Neither reuses loose WAV/DDS container layouts blindly.
- [ ] GameMaker texture pages, XNB, Ren'Py, RPG Maker and Source/Creation archives,
  ranked by measurable eligible bytes and format tractability, not engine hype.
- [ ] Only then selectively investigate proprietary families. An unsupported
  writer is an explicit safe skip, not a placeholder success.

### Acceptance gate for every writer

Unit/malformed-input fixtures → independent unchanged-resource verification →
export to new destination → byte/reference/mip/loop checks → repeated-apply
stability → physical Btrfs measurement where relevant → manual game-copy
playtest → documented bounded support → opt-in main-pipeline integration.
Compatibility and asset purpose come before savings. Logical size reduction,
referenced physical footprint and net free-space gain are reported separately.
Tested game copies are evidence for a format implementation, **not game-specific
rules**. Never launch or kill games automatically or edit installed games as tests.

## Versioning and incremental GitHub publication

- `v0.1.0`, `v0.1.1` and `v0.2.0` were tagged previously; the `v0.2.0` release
  build failed on aarch64, so no 0.2.0 artifact was published. `0.2.1` is the
  current **release**: script, native Cargo package/lock and manpage all say
  `0.2.1`. Engine asset writers inside it are beta; see [SUPPORT.md](SUPPORT.md).
- Keep a shared SemVer number in the script, native Cargo package/lock and manpage.
  Update them together at a release boundary, with a changelog and package checks.
  Use commit hashes to identify individual development improvements; not every
  commit deserves a version bump or tag.
- **Patch** (`0.2.1` after 0.2.0 ships): fixes/guards for existing supported formats.
  **Minor** (`0.3.0`, `0.4.0`): meaningful new bounded format capability. Before
  1.0, breaking CLI/protocol/state changes also require a minor bump and explicit
  migration notes. **1.0.0** means a stable contract, not “every engine supported”.
- An audit-only release may be useful, but must be labelled audit-only. Do not
  bump a version and advertise texture compression merely because we detect it.
- Push small, coherent, tested commits: baseline safeguards/audio, roadmap/docs,
  Unity reader, Unreal reader, export writers, then apply integration. Avoid
  monolithic “all engines supported” commits or partially working production routes.
- Normal pushes run CI; `v*` tags trigger the existing release workflow and
  public checksummed x86_64/aarch64 archives. Do not tag until those release gates
  pass. Published tags/archives are immutable; fixes get a new version.
- Publish implementation, synthetic fixtures and sanitized results, **not** game
  binaries/assets, credentials, personal manifests or local scratch outputs.

## Sources and limits of the quick investigation

Accessed 2026-10-02. Engine popularity is not equivalent to compressible bytes;
no live market-share percentage is assumed for planning.

- [VGI, Big Game Engine Report 2025](https://app.sensortower.com/vgi/assets/reports/The_Big_Game_Engines_Report_of_2025.pdf): Steam engine families and trends, based on its tagged/estimated dataset and 2024 results, not an exhaustive 2026 census.
- [Unity: build content output](https://docs.unity3d.com/Manual/build-content-output.html) and [UnityDataTools: player build format](https://github.com/Unity-Technologies/UnityDataTools/blob/main/Documentation/playerbuild-format.md): SerializedFiles, companion streams, default absence of player TypeTrees.
- [Epic: packaging](https://dev.epicgames.com/documentation/en-us/unreal-engine/packaging-your-project) and [Epic forum explanation of IoStore](https://forums.unrealengine.com/t/external-pak-files-and-use-io-store-option/2137290): cooked data, Pak versus IoStore and package addressing.
- [Godot: exporting packs](https://docs.godotengine.org/en/stable/tutorials/export/exporting_pcks.html), plus the tested local PCK/GDST/GST2 implementations.
- [Ren'Py: building distributions](https://www.renpy.org/doc/html/build.html): RPA packaging and script/save-identity constraints.
- [O3DE: packaging](https://o3de.org/docs/user-guide/packaging/): asset-bundling pipeline, not evidence that CryEngine/Lumberyard share exact formats.
- [SteamDB technology catalogue](https://steamdb.info/tech/): useful broader catalogue; direct automated access returned HTTP 403 during this survey, so no counts are taken from it.

Current support was checked against `native/src/{assets,engines,containers,
unityfs,gdst,gst2,godot3,fmod}.rs` and the recorded tests, not inferred from an
engine vendor's advertised features. Format/version-specific release evidence
belongs in `test/engine-results/`; update this chart as adapters progress.
