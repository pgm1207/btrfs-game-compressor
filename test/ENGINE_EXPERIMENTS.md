# Packed-engine studies (2026-10-01)

Goal: target bytes that filesystem compression misses, without spending SSD
endurance on negligible reductions. These studies read installed files; all
compression and duplicate planning happened in memory. **No installed assets
were replaced, no real-game candidate packs were written, and no new physical
storage savings are claimed.** Exports were exercised on disposable synthetic
fixtures, including every supported container layout.

## Target selection

The existing compression database (rounded MiB, historical rather than fresh
measurements) identifies MonsterHunterRise at about 0.32% saving, Dispatch 0.57%,
Phoenix Wright Ace Attorney Trilogy 1.70%, Valheim 3.97%, OCTOPATH TRAVELER 4.52%,
Disco Elysium 6.35%, and Sable 8.14%. Large Unity bundle and Godot titles were
also sampled to validate the format implementations and locate resource types.
History does not prove current physical footprint or incremental rewrite gains.

## UnityFS

The new native parser/exporter supports versions 6–8, stored/LZ4/LZ4HC metadata,
metadata before or after block data, and supported alignment flags. It verifies
declared extents, node bounds and every rebuilt block's decoded bytes. Block
boundaries, codec IDs, resource directories and decoded-data hashes are retained.
LZMA, unknown flags, nonzero layout padding and excessive resource budgets fail
closed. This is bundle recompression, **not Texture2D or virtual-texture rewriting**.

- Initial study: 10 selected Despelote/Dave the Diver bundles, 940,431,305 input
  bytes; **zero logical reduction** with LZ4 HC level 12.
- Low-yield study: 12 selected Phoenix Wright/Valheim/Sable bundles,
  784,096,916 input bytes; 186,980 bytes potential total logical reduction.
  The largest Phoenix Wright candidate saves 186,759 bytes (about 0.299% of
  its complete rewritten output). Every candidate fails the default 5%
  savings/output-write gate; the audit writes nothing.
- Valheim's three largest extensionless UnityFS bundles exceed the current
  512 MiB in-memory input limit and were **not** audited. Results are not
  extrapolated to them or to whole games.
- Disco Elysium's large `.vtc2` files are Amplify virtual-texture containers,
  not ordinary `.resS` streams; `.resource` can contain FSB5 audio. Engine
  inventories now distinguish these paths instead of hiding their bytes.

Reports: `engine-results/unityfs-initial-audit.json` and
`engine-results/unityfs-low-yield-audit.json`.

## Godot

The pack loader reads entries directly; arbitrarily adding a per-entry Zstd
codec flag is **not supported**. This corrects the earlier proposed approach.
The native inventory reads standalone PCK v1–3 directories with bounded sizes,
then samples GST2 texture headers and bounded `RSRC` headers for `.sample`
resources. `PCK_RESOURCE` reports resource class; `PCK_AUDIO` parses bounded
AudioStreamWAV properties and seeks over, rather than reads, its data array.
These inventories do not verify audio payload contents or every PCK entry hash.

The lossless exporter finds duplicate payloads using stored MD5/size as hints,
verifies actual equality byte-for-byte, and lets resource paths share an offset.
It removes only verified duplicate extents, retains other bytes/gaps, relocates
resource/directory offsets, and verifies **every final resource against its
source**. No decoder or per-frame work is introduced. Encrypted/sparse packs,
removal records, partial overlaps and unknown/newer layouts are unsupported.
Runtime compatibility still needs in-game validation before installed apply.

- 17 top-level installed PCK files examined; 11 supported and 6 rejected.
- Supported input total: 5,666,011,780 bytes. Potential duplicate-payload
  reduction: 14,183,351 bytes (13.53 MiB). **Every** candidate was below the
  5% savings/output-write gate or had no gain; no candidate file was written.
- Unsupported entries are reported explicitly, not treated as optimizable:
  Click the Button, Gamblers Table, MOLDRISE, Pathogenic, Project P.I.T.T and
  Until Then. Extension detection is not sufficient format support.

High-value resource findings:

| Game | Resource group | Logical entry bytes |
|---|---|---:|
| Dome Keeper | `.sample` audio | 742,674,164 |
| Dome Keeper | `.oggvorbisstr` | 372,021,926 |
| Slay the Spire 2 | `.ctex` textures | 1,222,719,762 |
| Slay the Spire 2 | `.fontdata` | 308,762,339 |
| Slay the Spire 2 | `.bank` audio | 316,192,166 |

The bounded Godot 4.3 binary-resource parser was exercised on Dome Keeper's
installed PCK. All 1,384 `.sample` entries (742,674,164 logical bytes) identify
as `AudioStreamWAV`; all report 16-bit PCM and their serialized `data` arrays
total 742,143,404 bytes. 143 resources have looping enabled. The rest of each
entry holds resource metadata/padding. Mix rates are preserved per resource
(most are 44.1 or 48 kHz; one is 22,257 Hz). These are format observations, not
transformation gains: there is no supported lossless smaller codec in this
resource path. Godot 4.3 can play QOA, but that encoding is lossy and its binary
resource saver does not support writing QOA; no QOA conversion is implemented.
Game loading and audio quality have not been validated.

For research only, streaming the complete PCK through Zstandard level 3 to
stdout (discarding the output) produced 1,006,454,888 bytes from 1,249,758,944
logical bytes (19.47% logical reduction); encoding the 742,143,404 extracted
PCM bytes as independent Zstandard frames produced 532,011,827 bytes (28.31%
reduction). These are **not** physical Btrfs savings and do not make an
installable PCK: Godot PCK entries expose encryption/removal flags, not a
per-resource Zstandard decoder. The output was discarded; no copy or installed
asset was written. A privileged Btrfs allocation measurement is still needed
to tell how much of this is incremental to the installed game's current state.

`raw-image` in GST2 is an image serialization mode, **not proof of uncompressed
pixels**: many Slay the Spire textures carry GPU-compressed numeric formats.
No texture is declared safely downscalable from screen resolution alone, and
no font/language resource is declared optional from its name or size.

Godot Image format IDs are now also named in read-only inventory output. For
Slay the Spire 2's Godot 4.5.1 PCK, the 516 `raw-image` format-19 textures use
DXT5/BC3 (210,976,528 entry bytes), and 683 format-22 textures use
BPTC_RGBA/BC7 (701,695,116 entry bytes). These payloads are already GPU block
compressed; resizing, transcoding or changing formats would be a quality or
runtime tradeoff, not a lossless byte repack. No such transform is implemented.

Slay the Spire 2 was selected for a read-only Ultra Performance trial. The loose
asset plan found **zero** eligible images, DDS files or WAVs: its 1.9 GB of
assets are in the Godot PCK. Verified PCK duplicate sharing would save
1,424,818 logical bytes (0.0750% of output), below the 5% write-efficiency gate,
so no export was written. Streaming the PCK through standalone Zstandard level
3 and discarding the output produced 1,106,982,873 bytes from 1,901,047,880
(41.77% logical reduction); this is only a codec benchmark, not a runnable PCK
or a physical-space claim. `du` reports 1,901,051,904 allocated bytes for the
installed PCK, but privileged Btrfs extent measurement is unavailable here.
**No game file was changed and no playability test is possible yet.** The next
game-specific implementation target is a validated export-only Godot texture
transform; BC7 decoding and PCK compatibility are unresolved.

Follow-up format research: Godot 4.5.1 `CompressedTexture2D` reads the GST2
header as `GST2`, version, 32-bit width/height/data-format/mipmap-limit and
three reserved words. Its image payload has a second header (encoding, 16-bit
dimensions, mip count and Godot image-format ID). WebP/PNG payloads are
length-prefixed per mip; `raw-image` payloads use Godot's image-format byte
layout. The official loader rejects malformed payloads and only permits
compressed decoded images when their declared format matches. A rewrite must
therefore regenerate GST2 mip data and retain a matching GPU format, or switch
the payload encoding to a supported image mode with a matching decoded format.

Research also identified `image_dds` (MIT, pure Rust API, BC1–BC7 decode and
BC7 encode through its bundled Intel texture encoder) and `bcdec_rs` (MIT) as
self-contained components. They are now linked into `bgc-native` together with
`intel_tex_2` for BC re-encoding, and the export-only texture transform is
implemented:

- `bgc-native godot-texture-audit PROFILE FILE` reports the logical size a
  rewrite would reach without writing anything.
- `bgc-native godot-texture-export PROFILE MIN_PCT INPUT OUTPUT` writes a new
  v3 PCK. Supported GST2 textures (WebP/PNG and BC1/BC2/BC3/BC7) are decoded,
  downscaled to the profile's longest edge and re-encoded in the same pixel
  format; the mip count is preserved and the chain regenerated. Unsupported
  encodings (Basis Universal, ETC, ASTC, half-float, RGB565/RGBA4444) and
  textures already within the edge limit are copied byte-for-byte. A texture is
  only replaced when the new payload is strictly smaller.
- The pack is rebuilt with 32-byte alignment, relocated relative offsets and
  recomputed per-entry MD5 (`native/src/md5.rs`); it is then re-parsed and every
  untouched entry is byte-compared against the source. Encrypted/sparse packs,
  removal records, aliased extents with mismatched sizes and non-v3 layouts are
  rejected.

This transform is the first one that changes stored texture pixels. It is
gated to the explicit lossy profiles (Native/Lossless do nothing) and remains
export-only: it never overwrites the installed pack, and **in-game loading of a
rewritten pack has not been validated and physical Btrfs savings are unmeasured**.
Fonts, Spine atlas data, animations and gameplay UI semantics are not specially
classified; the only policy is "supported encoding + above the edge limit".


Reports: `engine-results/godot-dedup-audit.json` and
`engine-results/container-initial-audit.json`.

## Godot 3 (PCK v1) — Brotato

Brotato 1.0 (Godot 3.7, standalone PCK v1, 7416 entries, 147,912,168 bytes) was
used as the first Godot 3 target. Its bytes are dominated by packed audio:
`.mp3str` 82,200,673 and `.sample` 18,171,860, with only 14,077,548 of `.stex`
(GDST) textures (1,260 of 1,261 are ≤256 px icons, several 96×96).

The new `godot3-optimize` path reads `RSRC` binary resources, decodes
`AudioStreamMP3` with a pure-Rust decoder and re-encodes Ogg Vorbis under the
same entry name, retyping the matching `.import` stub's `type=` (Godot 3 loads a
resource by its stored type, not its extension). Measured on the real pack:

| Profile | Output bytes | Saved | MP3 re-encoded |
|---|---:|---:|---:|
| ultra-performance (q 0.10) | 88,554,018 | 59,358,150 (40.1%) | 34/35 |
| performance (q 0.22) | 94,153,426 | 53,758,742 | 34/35 |
| balanced (q 0.35) | 101,375,133 | 46,537,035 | 33/35 |

Verification: the rebuilt PCK re-parses with the same 7,416 entries; all 33–34
rewritten resources re-parse as `AudioStreamOGGVorbis` with a `qoaf`/`OggS`
payload; every other entry (including all `.import` stubs that did not reference
a converted file) is byte-identical. One rewritten track extracted from the
candidate is a valid Ogg Vorbis stream (`ffprobe`: 195.5 s, 132 kbps) and decodes
to PCM at an RMS close to the source MP3 (0.247 vs 0.285 full-scale). No in-game
playback/looping validation has been run yet, and the `.sample`/`.stex` bytes
are left untouched. Logical reductions; no privileged Btrfs allocation
measurement was taken, though `du` allocation matches logical size for these
packs, so the reduction is expected to be physical.

Brotato's `.stex` textures are the Godot 3 `GDST` container (`GDST`, width,
height, DataFormat, 32-byte image header, then `WEBP`/`PNG` + payload). The
bundled image decoders can read the payloads, but the texture set is small and
would only save a few MB, so no `.stex` rewriting is implemented yet.


## Unreal

All 19 Dispatch Pak footers parsed as version 11 with encrypted indexes.
They declare Oodle; one also declares Zlib. Footer declarations are **not an
inventory of actual compressed entries**. No index decryption, key extraction,
Oodle encoder integration or repacking was attempted. IoStore and RE Engine
remain separate unsupported rewriting formats. Existing filesystem compression
must not repeatedly rewrite these archives based on a hypothetical codec swap.

Report: `engine-results/container-initial-audit.json`.

## Next genuine bottlenecks

These measurements make blind whole-container recompression a poor next step.
Higher-impact work requires actual resource transforms: Unity serialized
Texture2D/stream metadata and Amplify virtual-texture parsing; Godot sample
serialization and codec/loop contracts; or engine-aware, validated texture
changes. Keep each path export-only until byte/semantic checks, physical savings,
and game playback/loading validation justify installed replacement. Raw report
files preserve negative results so unsuccessful passes are not mistaken for
savings or repeated without a changed strategy.

Format references (not runtime dependencies):
- https://github.com/Perfare/AssetStudio/blob/master/AssetStudio/BundleFile.cs
- https://github.com/godotengine/godot/blob/4.5/core/io/file_access_pack.cpp
- https://github.com/godotengine/godot/blob/4.5/scene/resources/compressed_texture.cpp
- https://github.com/trumank/repak/blob/master/repak/src/footer.rs
