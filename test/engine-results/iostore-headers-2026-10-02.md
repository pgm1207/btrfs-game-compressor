# IoStore header inventory — 2026-10-02

Read-only `.utoc` header audit; no installed game data was changed, no game was
launched, and no `.ucas` payload or companion signature was opened.

## Capability

Content detection uses the 16-byte IoStore magic, not the filename extension.
The fixed 144-byte header reports TOC version (1–8), chunk/block counts,
compression block size, method-table dimensions, directory-index size,
partition metadata, container ID and security flags. Only metadata from regular
same-stem `.ucas`, `.sig` and `.pak` files is reported; companion links are ignored
and symlinked input TOCs are rejected.

Reference: [retoc reader](https://github.com/trumank/retoc/blob/master/retoc/src/lib.rs).
The minimum extent includes 12-byte IDs and two five-byte offset/length fields,
12-byte compression-block records, version-gated perfect-hash arrays, method
names, directory bytes and version-specific metadata (33 bytes before v8,
24 in v8). Signed containers add a length word and 20 bytes per block; their
variable-length signatures remain outside this minimum and unverified.

`UNREAL_IOSTORE_LAYOUT|file_bytes|minimum_bytes|residual_bytes` is a structural
bound, **not a checksum or authenticity check**. Extra bytes are not interpreted.
`UNREAL_IOSTORE_HASH` reports perfect-hash table counts, not verified hashes.

## Installed-library survey

- 69 TOCs accepted: 35 version 8 and 34 version 6.
- None declared encryption; 26 declared signing, 24 had regular `.sig` siblings.
  Embedded signing flags and companion presence are independent observations.
- 43 TOCs matched the minimum extent exactly. The 26 signed TOCs each had
  1,024 unparsed bytes beyond it; these signatures were not read or verified.
- Largest declared chunk count: 86,297.
- Satisfactory main TOC: 59,158 chunks, 265,151 blocks, 64 KiB block size,
  1,685,273 directory-index bytes; compressed/signed/indexed flags. Its regular
  `.ucas` sibling reports 7,885,556,928 bytes. None of those bytes was read.

Header fields and file lengths establish inventory only. Chunk IDs, addressing,
block references, package-store/Zen schemas and Texture2D exports remain future
work. There is no IoStore repacker or Unity/Unreal texture writer.

## Tests

Synthetic coverage includes truncated headers, unknown versions/flags, hostile
counts and structural fields, version-specific minimum bounds, signed overhead,
content-detected CLI routing, unchanged source/companion bytes, impossible
extents and input/companion symlink handling. Validation: 119 unit tests,
7 integration tests with Btrfs fixtures enabled, 433 smoke checks and a static
release package build passed. No release tag was created.
