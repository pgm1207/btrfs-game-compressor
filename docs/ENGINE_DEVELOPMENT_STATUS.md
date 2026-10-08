# Engine reader drafts — not validated support

Updated **2026-10-08**. The baseline drafts were committed in `859cb4d`; they
are not a new release.

The **current 2026-10-08 revision is now built and tested** in isolated target
directories (the repository `bgc-native` and the frozen installed-library runner
were **not** rebuilt or replaced): default **159 unit + 9 filesystem** tests pass;
`development-audits` **158 unit + 9 filesystem** tests pass; the Python suite is
**16 tests OK**; `test/smoke.sh` is **483 passed / 0 failed**. Real read-only
audits reproduced the Carrion XNB tally and ran Cocoon bundle inventories; four
detached Carrion exports used the new anonymous-publication path with unchanged
sources and independent Pillow verification (36.0–50.7 dB). The default binary
reports `Development reader routes: disabled`, the feature binary `compiled in`.
Full evidence: [2026-10-08 review](ENGINE_HARDENING_REVIEW_2026_10_08.md).

The 2026-10-08 review adds tested-source hardening for immutable XNB reparsing,
anonymous atomic detached publication, separate experimental report failures,
independent verifier bounds, and mixed Unity tree diagnostics. Passing tests are
**local development results, not runtime certification or a support promotion**;
none of the drafts below is promoted to tested audit support, beta export or beta
apply.

The **2026-10-07 baseline** recorded isolated offline `cargo check` and `cargo
test` runs for both configurations: 150 native unit and 9 integration tests per
configuration. Baseline tests cover source read budgets/change detection,
component-wise companion opening, record escaping, XNB export and reader bounds,
and Unity texture shapes.

The format research is in [ENGINE_COMPATIBILITY_NEXT.md](ENGINE_COMPATIBILITY_NEXT.md).
[SUPPORT.md](../SUPPORT.md) remains the evidence-based support contract. None of
the drafts below is promoted to tested audit support, beta export or beta apply.

## Draft boundaries

These **native-only** command names are registered in source, with no Bash UI,
container auto-routing or installed-library integration. They are not available
in published 0.2.1 binaries. Their dispatch is additionally gated behind an
explicit `development-audits` Cargo feature, **off by default**. Normal source
builds refuse these commands without opening the supplied input.
`--development-audits` lists source-level routes and the compile-time gate state.
The tested guard is a local development result, not a release guarantee.

| Draft route | Source | Metadata subset | Still opaque / not supported |
| --- | --- | --- | --- |
| `xnb-texture-audit FILE` | `native/src/xnb.rs` | Uncompressed v5, `w`/`d` targets; bounded reader table and exact allowlisted root Texture2DReader, including its bare built-in name; Color/Dxt1/Dxt3/Dxt5 mip extents | LZX/LZ4, v4/v6 payloads, shared resources, custom readers, SpriteFont, pixel/channel/alpha semantics, external atlas ownership |
| `xnb-texture-export MAX_EDGE INPUT OUTPUT` | `native/src/xnb.rs` | Development-only detached resize export for a single-mip, uncompressed v5 BC1/2/3 Texture2D root; source preserved, byte-identical no-change rebuild checked, BC1 alpha mask retained, output re-parsed | No installed apply, runtime proof, atlas/reference safety, multi-mip writer or physical-savings claim |
| `vtf-audit FILE` | `native/src/vtf.rs` | Conservative PC 7.1–7.4 headers; resources, thumbnail bounds and single-frame ordinary 2D common storage layouts, smallest-mip-first extents | Animation/cubes/volumes, streamed/procedural layouts, unknown formats/flags, material/sheet ownership, pixel checks and resource integrity |
| `vpk-audit FILE` | `native/src/vpk.rs` | Standard Valve v1/v2 tree, preload extents, embedded data bounds, declared numbered-archive requirements and extension totals | Numbered archive bytes, CRC/MD5/signatures, entry payloads, custom Respawn VPK, Source 2 compiled resources, repacking |
| `gamemaker-audit FILE` | `native/src/gamemaker.rs` | Exact FORM chunk framing; GEN8/STRG markers as a candidate, TXTR/TPAG candidate count/pointer bounds and alias/null/cross-chunk risks | Runtime/version detection, pointer-object schemas, page pixels, atlas coordinates, external texture groups, sprites/fonts/code/audio |
| `unityfs-texture-inventory FILE` | `native/src/unityfs.rs`, `unity_serialized.rs`, `unity_tree.rs` | Supported bundle directory/blocks; selected SerializedFile nodes; bounded Texture2D values and selected original field spans; retained external identities, object table/ranges, same-bundle stream bounds and alias/overlap summaries | Stripped/unknown trees, unresolved external PPtrs, Sprite/SpriteAtlas/mesh ownership, purpose/color/alpha checks, texture decode, object/bundle rebuilding and catalog/provider integrity |
| `iostore-index-audit FILE` | `native/src/iostore_index.rs` | Unsigned/unencrypted v3–8 addressing tables, chunk IDs, block/method/partition declarations, perfect-hash table framing and directory/metadata section bounds | Perfect-hash lookup verification, directory contents, chunk hashes, `.ucas` opening, decompression, Zen/package schemas, proprietary codecs and repacking |
| `godot4-audio-audit FILE` | `native/src/godot4_audio.rs` | Standalone little-endian Godot 4.3 RSRC format 6 tables/identities; one reference-free AudioStreamWAV; allowlisted property variants, PCM frame alignment and loop bounds | RSCC, other versions, big-endian/real64, unknown properties, scripts/external/multiple resources, Vorbis packet sequences, ADPCM/QOA frame parsing, audio decode and PCK audio writing |

The storage formulas use engine-specific format-ID maps. The Unity detailed
inventory now checks RGBA32/BGRA32, DXT1/DXT5 and BC4/BC5/BC7 2D mip byte
shapes, using [Unity's format IDs](https://github.com/Unity-Technologies/UnityCsReference/blob/master/Runtime/Export/Graphics/GraphicsEnums.cs)
and [Microsoft's BC block sizes](https://learn.microsoft.com/en-us/windows/win32/direct3d11/texture-block-compression-in-direct3d-11).
The shared `Layout` type describes byte/block shape only: it does not convert
engine IDs, certify channel order or infer a texture's purpose.

## Resource limits and safety intent

Limits below describe the source implementation, **not measured performance or
tested peak memory**. Every draft takes one explicitly supplied regular file;
none scans a library or follows an archive entry onto the host filesystem.

| Reader | Declared file / work boundaries |
| --- | --- |
| XNB audit | At most `u32::MAX` file bytes; 1 MiB actual read budget; 128 readers, 4096 bytes per reader name; mip pixels are range-checked and skipped |
| XNB detached export | At most 64 MiB source; one BC1/2/3 mip and at most 16,777,216 pixels; source and candidate pixels decoded; immutable source/candidate reparsing; unvalidated anonymous staging and no-replace publication draft |
| VTF | At most 4 GiB file length; 64 KiB actual reads; 32 resources; image/resource bodies are not decoded |
| VPK | At most 8 GiB file length; 32 MiB tree; tree + 64-byte read budget; 65,536 entries; 8 MiB retained path/extension text budget; at most 4096 detailed entry records |
| FORM | At most `u32::MAX + 8` file bytes; 1 MiB actual reads; 256 chunks, 4096 candidate pointers per table; no object dereference |
| Unity detailed inventory | Source and declared decoded data each at most 128 MiB; 4096 nodes; only exact `SerializedFile` flag 4 selected; 128 MiB aggregate block-decode work, 64 MiB aggregate selected-node copies, one cached block at a time; 32 MiB aggregate metadata and 64 MiB object walking, at most 10,000 object reports; conservative 8 MiB report-size budget |
| IoStore | At most 256 MiB TOC; 8 MiB actual reads; 65,536 chunks/blocks, 131,072 seeds, at most 4096 detailed chunk/block records; no companion reads |
| Godot audio | At most 512 MiB resource file; 1 MiB actual reads; 4096 strings, 1024 external/internal identities, 128 allowlisted properties; packed audio bytes are range-checked and skipped |

Unity still reads the supplied compressed bundle into memory. Selected nodes
are assembled from intersecting decoded blocks instead of retaining the entire
decoded bundle, but a block may contain unrelated stream bytes too. This reader
can still perform substantial CPU/memory work; **read-only is not lightweight**.
Decoded work, node copying, metadata walking and report size are separate limits,
not one combined peak-memory guarantee. It does not validate unselected blocks.

Common draft primitives (`native/src/audit_io.rs`) intend to provide:

- Descriptor-based reads, overflow/range/read-budget checks, cancellation, and
  opened-inode size/time change detection. This is **not** a filesystem snapshot.
- Final source-symlink refusal and regular-file checks. The user-supplied source
  path's ancestor directories are not a sandbox or a trusted-root guarantee.
- Reversible percent escaping of untrusted record fields, including `%`, `|`,
  backslashes, control characters and non-ASCII bytes.
- An `openat` companion helper anchored to an open directory, refusing symlinks
  on each relative component. The local standalone Unity resolver now uses this
  draft helper for metadata-only bounds checks. It does not establish that the
  initial directory is trusted, freeze companion contents, forbid directory
  relocation or mount changes,
  or prove stream ownership. No stream payload reads were added.

Shared Unity source reading also has a local bounded-allocation/chunked-
cancellation draft. This and the standalone companion hardening touch existing
source paths, so future validation must include their old inventory/export
contracts as well as the new feature-gated routes. The feature guard does not
mean every shared source change is excluded from normal builds.
Local Unity audit/coverage paths also now propagate interruption instead of
counting an interrupted inspection as opaque, and coverage rechecks opened-source
size/time metadata before returning counts. The baseline compiled and passed its
existing tests; the latest shared changes need both build configurations and
focused regressions. Tree parsing now checks bounded hierarchy/local strings
before unknown common-string semantics; detailed statuses distinguish
`MALFORMED_TREE`, `UNSUPPORTED_TREE_SCHEMA`, `TREE_BUDGET_SKIPPED` and
`TREE_PARSE_ERROR`. Interruptions propagate. This new diagnostic contract is
provisional and untested, and does not prove that an opaque object is safe.

## Output is evidence, not eligibility

- `METADATA_ONLY` means only the named subset's metadata was inspected; it is
  not pixel/audio validity, successful playback, optimization eligibility or
  fully verified container integrity.
- `OPAQUE` and named budget/schema statuses are skips, not successes. A recognized
  header can still have entirely unread payloads. Numeric file/read/allocation
  limit errors can also abort with a nonzero result before records are emitted.
- Unity node/object/field spans refer to **original bytes only**. Object starts
  and object-table spans are relative to the SerializedFile node; field spans
  are object-relative, exclude trailing alignment, and retain length-prefix and
  payload boundaries separately. They are not a safe editing API. IDs retain
  their original 64-bit bit pattern; PPtr reference indexes are 1-based.
- Known Unity mip-shape checks require explicit `m_ImageCount == 1` and
  `m_TextureDimension == 2` as well as a known codec. Missing shape metadata is
  not assumed to be a single 2D image. `.resS` stream ranges remain
  `OWNERSHIP_UNPROVEN`, including ranges that have valid bounds.
- Unity declared streamed totals can count aliases repeatedly. The range union
  counts only in-bounds references into permitted same-bundle resource nodes;
  it is not an ownership proof, a stream-file size reduction, or eligible savings.
- VPK logical totals include preload plus body lengths for every entry, not
  physical storage savings. Required companion lengths are declarations, not
  measurements of companion existence, contents or integrity.
- IoStore method names do not enable codecs. Chunk/block addressing and partition
  requirements do not establish companion availability or correct cooked assets.
  Per-partition range sums/unions and alias/overlap counts describe declared
  compressed-block offsets only, not validated companion contents or savings.
- Godot ADPCM/QOA data sizes are not inferred decoded frames. Even PCM byte/frame
  arithmetic does not establish waveform validity or loop playback semantics.

Only the bounded XNB subset has a **detached development export**. No draft has
an installed writer or automatic routing. Existing beta writers, support levels,
release numbers and the paused compaction journal are unchanged.
The local package recipe now includes `SUPPORT.md` and `docs/` alongside the
existing roadmap/scope files so those links can survive a future source archive.
No package was built and no existing archive was changed; package contents and
documentation links require deferred validation too.

## Next implementation work

1. Continue source-level review of resource bounds, version traps and cancellation;
   keep report schemas explicitly provisional.
2. Validate the mixed malformed/unknown-tree diagnostic draft, then implement
   Sprite/SpriteAtlas/PPtr consumers without guessing stripped schemas. Absent
   trees and retained per-type budget/schema statuses are already distinct drafts.
   [UNITY_REFERENCE_OWNERSHIP_CONTRACT.md](UNITY_REFERENCE_OWNERSHIP_CONTRACT.md)
   defines graph identity, completeness and consumer gates; no graph reader or
   ownership proof is implemented yet.
3. Define no-change rebuilding and retained-byte contracts for an exact inline
   Unity version/schema. The experimental XNB export now checks a byte-identical
   no-change rebuild; it still needs independent pixel comparison and runtime
   checks before support promotion.
   [Export gates](ENGINE_EXPORT_GATES.md) defines the remaining contract.
4. Establish VPK entry reconstruction (preload + body), Godot resource ownership,
   and IoStore companion/directory constraints before any export/decompression.
5. Investigate other bounded archive families only after documenting their
   version, integrity and reference requirements; detection is not engine support.
   [Follow-up candidates](ENGINE_FOLLOWUP_CANDIDATES.md) records preliminary
   BA2, Wwise and Defold findings without adding more reader routes.

## Remaining validation

Baseline isolated builds, native tests, a default-vs-feature command guard check,
and three shared-I/O fixtures were recorded earlier. The 2026-10-08 revision has
now additionally been built (both configurations) and its native/Python/smoke
suites pass, with real Carrion XNB and Cocoon Unity read-only audits and four
verified detached exports; see the
[2026-10-08 review](ENGINE_HARDENING_REVIEW_2026_10_08.md). Continue with the
remaining focused fixtures and independent reconstruction work:

- Recheck both build configurations after substantive changes, using separate
  build/output locations without replacing `bgc-native` or the frozen installed-run
  backend. Default builds must refuse draft commands before source opening;
  feature builds must expose no installed writer or automatic routing.

- Shared I/O: final/intermediate symlinks, traversal, FIFO/nonregular files,
  depth/read/allocation budgets, escaped record round trips and cancellation.
- XNB: 7-bit overflow/overlong encodings, qualifier allowlist, shared/null/custom
   roots, targets/versions, mip arithmetic, truncated blobs and trailing data.
  Also validate snapshot/descriptor agreement, retained prefix, partial-block BC1
  alpha, anonymous publication/destination races, source-guard aborts, unsupported
  staging filesystems and directory-sync failure after commit. Independent Python
  fixtures and report failure/timeout/coverage fixtures now run in the Python
  suite; anonymous-publication commit-point edge cases still need dedicated
  disposable-directory fixtures.
- VTF/VPK: independent header/tree framing, mip order, resource flags/overlaps,
  preload-only entries, section bounds, duplicate/case-colliding paths, malicious
  names and custom versions. Independently reconstruct entry bytes before
  composing VPK and VTF readers.
- FORM: exact chunk-length traversal, pointer-table budgets/nulls/aliases and
  chunk padding, before any runtime-specific TXTR/TPAG parsing.
- Unity: independent table/span comparison, endian/alignment/version checks,
  selected-block boundary/caching/work-budget fixtures, control characters,
  exact namespace matching, duplicate/aliased nodes and overlapping streams.
- IoStore: independently verify the opposite endian encodings of chunk and block
  offsets, versioned section strides, short block boundaries, partition crossings
  and overflow-table indexes; companion bytes and package interpretation stay
  separate future gates.
- Godot: independent RSRC/string/variant offsets, defaults, skipped packed arrays,
  identity/reference tables, unknown versions/properties and PCM/loop bounds.

Only after these readers are independently validated should disposable-source
audits and no-change/export rebuilding begin. Runtime checks, source preservation,
repeat stability and recovery remain separate requirements before beta apply.
Keep draft routes unreleased until their format-specific gates are satisfied.
