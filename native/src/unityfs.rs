//! UnityFS container work. The recompression route is export-only and lossless:
//! no resource extraction, codec changes, mip removal or edits to Addressables
//! catalogs/CRC records. The gated development routes below add a detached,
//! research-only Texture2D export and a verified in-place payload replace, which
//! can shrink a mip chain inside the object's unchanged serialized extent (inline
//! storage: zero-padded tail) or inside the uniquely-claimed `.resS` extent of a
//! streamed texture. A streamed shrink physically compacts the `.resS` node (the
//! freed tail `[new_end, old_end)` is removed from the decoded bytes, every later
//! node offset and every proven later stream offset shifts down by the delta);
//! when any reference cannot be proven the route falls back to the prior
//! zero-padded in-place rewrite. See `texture_replace` and
//! `replace_streamed_texture`. Layout reference: AssetStudio BundleFile.cs
//! (format documentation only).
use lz4::block::{compress, decompress, CompressionMode};
use std::{fs, io::{self, Read, Write}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};
use super::invalid;

const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_INFO: usize = 16 * 1024 * 1024;
const MAX_BLOCK: usize = 64 * 1024 * 1024;
const MAX_DECODED: u64 = 2 * 1024 * 1024 * 1024;

struct Cursor<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| invalid("UnityFS offset overflow"))?;
        let b = self.bytes.get(self.pos..end).ok_or_else(|| invalid("truncated UnityFS data"))?;
        self.pos = end; Ok(b)
    }
    fn u16(&mut self) -> io::Result<u16> { Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    fn u32(&mut self) -> io::Result<u32> { Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> io::Result<u64> { Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap())) }
    fn string(&mut self) -> io::Result<&'a [u8]> {
        let tail = self.bytes.get(self.pos..).ok_or_else(|| invalid("invalid UnityFS string offset"))?;
        let n = tail.iter().take(4097).position(|b| *b == 0).ok_or_else(|| invalid("unterminated/oversized UnityFS string"))?;
        self.take(n + 1)
    }
}

#[derive(Clone)]
struct Block { decoded: usize, encoded: usize, flags: u16, offset: usize, size_field: usize }
#[derive(Clone)]
struct Node { offset: u64, size: u64, flags: u32, path: Vec<u8> }
struct Bundle {
    version: u32, fields: usize, header_end: usize, flags: u32,
    info: Vec<u8>, blocks: Vec<Block>, nodes: Vec<Node>, decoded: u64,
}
fn align(n: usize) -> io::Result<usize> {
    Ok(n.checked_add(15).ok_or_else(|| invalid("UnityFS alignment overflow"))? & !15)
}
fn padding(b: &[u8], start: usize, end: usize) -> io::Result<()> {
    if b.get(start..end).ok_or_else(|| invalid("truncated UnityFS padding"))?.iter().any(|v| *v != 0) {
        return Err(invalid("nonzero UnityFS padding; unsupported layout"));
    }
    Ok(())
}
fn decode(b: &[u8], size: usize, codec: u32) -> io::Result<Vec<u8>> {
    let out = match codec {
        0 => b.to_vec(),
        2 | 3 => decompress(b, Some(size as i32))?,
        _ => return Err(invalid("unsupported UnityFS codec (only stored/LZ4/LZ4HC supported)")),
    };
    if out.len() != size { return Err(invalid("UnityFS decoded length mismatch")); }
    Ok(out)
}
fn parse(b: &[u8]) -> io::Result<Bundle> {
    let mut c = Cursor { bytes: b, pos: 0 };
    if c.string()? != b"UnityFS\0" { return Err(invalid("not a UnityFS bundle")); }
    let version = c.u32()?;
    if !(6..=8).contains(&version) { return Err(invalid("unsupported UnityFS version")); }
    c.string()?; c.string()?;
    let fields = c.pos;
    if c.u64()? != b.len() as u64 { return Err(invalid("UnityFS declared file size mismatch")); }
    let compressed_info = c.u32()? as usize;
    let decoded_info = c.u32()? as usize;
    let flags = c.u32()?;
    if flags & !0x2ff != 0 || flags & 0x100 != 0 || flags & 0x40 == 0 {
        return Err(invalid("unsupported UnityFS header flags"));
    }
    if compressed_info == 0 || compressed_info > MAX_INFO || decoded_info < 24 || decoded_info > MAX_INFO {
        return Err(invalid("UnityFS metadata exceeds limits"));
    }
    let header_end = c.pos;
    let header_aligned = if version >= 7 { align(c.pos)? } else { c.pos };
    padding(b, header_end, header_aligned)?;
    let at_end = flags & 0x80 != 0;
    let info_start = if at_end {
        b.len().checked_sub(compressed_info).ok_or_else(|| invalid("invalid UnityFS metadata offset"))?
    } else { header_aligned };
    if info_start < header_aligned { return Err(invalid("overlapping UnityFS header/metadata")); }
    let mut data_start = if at_end { header_aligned } else {
        info_start.checked_add(compressed_info).ok_or_else(|| invalid("UnityFS metadata overflow"))?
    };
    if flags & 0x200 != 0 { let end = align(data_start)?; padding(b, data_start, end)?; data_start = end; }
    let info_encoded = b.get(info_start..info_start + compressed_info).ok_or_else(|| invalid("truncated UnityFS metadata"))?;
    let info = decode(info_encoded, decoded_info, flags & 0x3f)?;
    let mut i = Cursor { bytes: &info, pos: 16 }; // Preserve the uncompressed-data hash.
    let count = i.u32()? as usize;
    if count == 0 || count > 65536 || count > (info.len() - 24) / 10 {
        return Err(invalid("invalid UnityFS block count"));
    }
    let mut blocks = Vec::with_capacity(count);
    let mut offset = data_start;
    let mut decoded = 0u64;
    for _ in 0..count {
        let uncompressed = i.u32()? as usize;
        let size_field = i.pos;
        let compressed = i.u32()? as usize;
        let bf = i.u16()?;
        if uncompressed == 0 || uncompressed > MAX_BLOCK || compressed == 0 || compressed > MAX_BLOCK + MAX_BLOCK / 255 + 16
            || bf & !0x7f != 0 || !matches!(bf & 0x3f, 0 | 2 | 3) {
            return Err(invalid("unsupported/oversized UnityFS block"));
        }
        if bf & 0x3f == 0 && compressed != uncompressed { return Err(invalid("invalid stored UnityFS block length")); }
        decoded += uncompressed as u64;
        if decoded > MAX_DECODED { return Err(invalid("UnityFS decoded work limit exceeded")); }
        blocks.push(Block { decoded: uncompressed, encoded: compressed, flags: bf, offset, size_field });
        offset = offset.checked_add(compressed).ok_or_else(|| invalid("UnityFS block offset overflow"))?;
    }
    let data_end = if at_end { info_start } else { b.len() };
    if offset != data_end { return Err(invalid("UnityFS block extents do not exactly cover data region")); }
    let node_count = i.u32()? as usize;
    if node_count == 0 || node_count > 65536 || node_count > (info.len() - i.pos) / 21 { return Err(invalid("invalid UnityFS node count")); }
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let off = i.u64()?;
        let size = i.u64()?;
        let node_flags = i.u32()?; // Retained as data; not interpreted as writer permission.
        let path = i.string()?;
        if path.len() <= 1 || off.checked_add(size).filter(|end| *end <= decoded).is_none() {
            return Err(invalid("invalid UnityFS resource extent/name"));
        }
        nodes.push(Node { offset: off, size, flags: node_flags, path: path[..path.len() - 1].to_vec() });
    }
    if i.pos != info.len() { return Err(invalid("unexpected trailing UnityFS metadata")); }
    Ok(Bundle { version, fields, header_end, flags, info, blocks, nodes, decoded })
}

fn rebuild(b: &[u8]) -> io::Result<(Vec<u8>, usize, Bundle)> {
    rebuild_patches(b, &[])
}

/// Container rebuild with an optional single byte-range patch in the decoded
/// bundle address space. `None` is the original lossless recompression;
/// `Some(patch)` is used by the detached inline texture replace, which changes
/// one SerializedFile node only.
#[cfg(any(test, feature = "development-audits"))]
fn rebuild_patched(b: &[u8], patch: Option<(usize, &[u8])>) -> io::Result<(Vec<u8>, usize, Bundle)> {
    match patch {
        Some(patch) => rebuild_patches(b, &[patch]),
        None => rebuild_patches(b, &[]),
    }
}

/// Container rebuild with zero or more disjoint byte-range patches in the decoded
/// bundle address space. No patches is the original lossless recompression. With
/// patches, only blocks that intersect a patch are decoded and re-encoded (with
/// their original codec, even when that grows them); every other block keeps its
/// source bytes verbatim. Patches must not overlap or change total decoded
/// lengths, so the block table's uncompressed sizes, node extents and the
/// metadata hash field stay valid by construction. The detached streamed texture
/// replace uses exactly two patches (the target object's fields in the
/// SerializedFile node and the `.resS` extent) in one pass, so each node's codec
/// is preserved and the block/node tables and info section are rebuilt exactly
/// once. Final verification compares each rebuilt block against the original
/// decoded content with every patch applied, so unrelated data is still proven
/// unchanged.
fn rebuild_patches(b: &[u8], patches: &[(usize, &[u8])]) -> io::Result<(Vec<u8>, usize, Bundle)> {
    let bundle = parse(b)?;
    let mut ranges = Vec::with_capacity(patches.len());
    for (start, bytes) in patches {
        let end = start.checked_add(bytes.len()).ok_or_else(|| invalid("UnityFS patch extent overflow"))?;
        if end > bundle.decoded as usize { return Err(invalid("UnityFS patch exceeds decoded bundle data")); }
        ranges.push((*start, end, *bytes));
    }
    ranges.sort_unstable_by_key(|(start, _, _)| *start);
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid("UnityFS patches overlap"));
    }
    let mut info = bundle.info.clone();
    let mut payload = Vec::new();
    let mut changed = 0;
    let mut block_start = 0usize;
    for block in &bundle.blocks {
        super::cancelled()?;
        let block_end = block_start.checked_add(block.decoded).ok_or_else(|| invalid("UnityFS block extent overflow"))?;
        let source = &b[block.offset..block.offset + block.encoded];
        let codec = (block.flags & 0x3f) as u32;
        let encoded = if ranges.is_empty() {
            let decoded = decode(source, block.decoded, codec)?;
            if codec == 0 { source.to_vec() } else {
                let candidate = compress(&decoded, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?;
                super::cancelled()?;
                if candidate.len() < source.len() {
                    if decode(&candidate, block.decoded, codec)? != decoded {
                        return Err(invalid("UnityFS block failed round-trip verification"));
                    }
                    changed += 1; candidate
                } else { source.to_vec() }
            }
        } else if !ranges.iter().any(|(start, end, _)| *start < block_end && block_start < *end) {
            // Untouched blocks keep their original compressed bytes exactly.
            source.to_vec()
        } else {
            let mut decoded = decode(source, block.decoded, codec)?;
            for (start, end, patch_bytes) in &ranges {
                let first = (*start).max(block_start);
                let last = (*end).min(block_end);
                if first < last {
                    let destination = first - block_start;
                    let offset = first - *start;
                    decoded[destination..destination + (last - first)]
                        .copy_from_slice(&patch_bytes[offset..offset + (last - first)]);
                }
            }
            if codec == 0 { decoded } else {
                // The block content changed, so its encoding must be used even
                // when it is larger than the source encoding.
                let candidate = compress(&decoded, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?;
                super::cancelled()?;
                if decode(&candidate, block.decoded, codec)? != decoded {
                    return Err(invalid("UnityFS block failed round-trip verification"));
                }
                changed += 1;
                candidate
            }
        };
        info[block.size_field..block.size_field + 4].copy_from_slice(&(encoded.len() as u32).to_be_bytes());
        payload.extend_from_slice(&encoded);
        block_start = block_end;
    }
    // Do not change any codec IDs or flags, including the original LZ4 vs HC ID.
    // The 16-byte uncompressed-data hash field is copied verbatim (mirroring the
    // lossless recompression route); callers must gate on whether that is valid.
    let codec = bundle.flags & 0x3f;
    let metadata = if codec == 0 { info.clone() } else {
        compress(&info, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?
    };
    if decode(&metadata, info.len(), codec)? != info { return Err(invalid("UnityFS metadata round-trip mismatch")); }
    let mut out = b[..bundle.header_end].to_vec();
    if bundle.version >= 7 { out.resize(align(out.len())?, 0); }
    if bundle.flags & 0x80 == 0 { out.extend_from_slice(&metadata); }
    if bundle.flags & 0x200 != 0 { out.resize(align(out.len())?, 0); }
    out.extend_from_slice(&payload);
    if bundle.flags & 0x80 != 0 { out.extend_from_slice(&metadata); }
    let length = out.len() as u64;
    out[bundle.fields..bundle.fields + 8].copy_from_slice(&length.to_be_bytes());
    out[bundle.fields + 8..bundle.fields + 12].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    let check = parse(&out)?;
    if check.info != info || check.flags != bundle.flags || check.decoded != bundle.decoded
        || check.blocks.len() != bundle.blocks.len() {
        return Err(invalid("rebuilt UnityFS layout failed verification"));
    }
    // Check the finished container, not just individual candidate buffers. Each
    // original block is decoded and every patch is applied before comparison, so
    // patched content is verified and every other byte is required unchanged.
    let mut block_start = 0usize;
    for (old, new) in bundle.blocks.iter().zip(&check.blocks) {
        super::cancelled()?;
        let mut expected = decode(&b[old.offset..old.offset + old.encoded], old.decoded, (old.flags & 0x3f) as u32)?;
        let block_end = block_start + old.decoded;
        for (start, end, patch_bytes) in &ranges {
            let first = (*start).max(block_start);
            let last = (*end).min(block_end);
            if first < last {
                expected[first - block_start..last - block_start]
                    .copy_from_slice(&patch_bytes[first - *start..last - *start]);
            }
        }
        let actual = decode(&out[new.offset..new.offset + new.encoded], new.decoded, (new.flags & 0x3f) as u32)?;
        if expected != actual { return Err(invalid("rebuilt UnityFS content differs")); }
        block_start += old.decoded;
    }
    Ok((out, changed, bundle))
}

/// One planned output block of a compacted rebuild: source-encoded bytes copied
/// verbatim (decoded content unchanged) or content that must be encoded again
/// (a partially removed block or one that carries a patch).
#[cfg(any(test, feature = "development-audits"))]
enum CompactBlock {
    Copied { old: usize },
    Built { decoded: Vec<u8>, old: usize },
}

/// Statistics of one compacted rebuild: the physically removed decoded length,
/// the new decoded total and, for every re-encoded block, its source index and
/// old/new encoded sizes (the caller reports them honestly; compression means a
/// removed decoded byte does not equal a removed file byte).
#[cfg(any(test, feature = "development-audits"))]
struct CompactionStats {
    removed: usize,
    new_decoded: u64,
    touched: Vec<(usize, usize, usize)>,
}

/// Container rebuild that PHYSICALLY drops the decoded byte range `[from, to)`
/// from the bundle address space and moves every later decoded byte down by the
/// removed length. Unlike `rebuild_patches` this changes the decoded total, so
/// the block table, every shifted node offset and the target node's size are
/// rebuilt rather than kept byte-stable:
///   * blocks that do not intersect `[from, to)` keep their source bytes
///     verbatim (their decoded content is unchanged, only its absolute offset
///     moved) unless a patch intersects them;
///   * a block that straddles `from` or `to` contributes only its kept piece(s),
///     each re-encoded with the block's own codec (a block wholly inside the
///     removed range contributes nothing and disappears);
///   * the metadata section is rebuilt with the same 16-byte hash placeholder,
///     the new block triples and the same node order/paths, with the target
///     node's size and every later node's offset adjusted.
/// `patches` are absolute ranges in the NEW decoded address space (after the
/// removal), must not overlap, must not straddle `from` (a patch cannot span the
/// cut) and are spliced verbatim. Every new block is decoded back and compared
/// against its expected content (the source block, or the exact piece/patch
/// splice), so no unrelated byte can change. Returns the candidate, its stats,
/// the source bundle and the reparsed candidate bundle.
#[cfg(any(test, feature = "development-audits"))]
fn rebuild_compacted(b: &[u8], patches: &[(usize, &[u8])], from: usize, to: usize, target: usize)
    -> io::Result<(Vec<u8>, CompactionStats, Bundle, Bundle)> {
    let bundle = parse(b)?;
    let decoded = bundle.decoded as usize;
    if from >= to || to > decoded { return Err(invalid("UnityFS compaction extent is invalid")); }
    let removed = to - from;
    let new_decoded = decoded - removed;
    let node = bundle.nodes.get(target).ok_or_else(|| invalid("UnityFS compaction target node is out of range"))?;
    let node_start = node.offset as usize;
    let node_end = node_start.checked_add(node.size as usize).ok_or_else(|| invalid("UnityFS compaction node extent overflow"))?;
    if from <= node_start || to > node_end { return Err(invalid("UnityFS compaction extent is outside the target node")); }
    // Every other node must lie wholly before the removal or wholly after it:
    // anything intersecting it could neither keep its bytes nor be shifted
    // unambiguously.
    for (index, other) in bundle.nodes.iter().enumerate() {
        if index == target { continue; }
        let start = other.offset as usize;
        let end = start.checked_add(other.size as usize).ok_or_else(|| invalid("UnityFS node extent overflow"))?;
        if !(end <= from || start >= to) {
            return Err(invalid("UnityFS compaction extent intersects a different node"));
        }
    }
    let mut ranges = Vec::with_capacity(patches.len());
    for (start, bytes) in patches {
        let end = start.checked_add(bytes.len()).ok_or_else(|| invalid("UnityFS patch extent overflow"))?;
        if end > new_decoded { return Err(invalid("UnityFS patch exceeds decoded bundle data")); }
        if *start < from && end > from { return Err(invalid("UnityFS patch straddles the compacted cut")); }
        ranges.push((*start, end, *bytes));
    }
    ranges.sort_unstable_by_key(|(start, _, _)| *start);
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) { return Err(invalid("UnityFS patches overlap")); }
    // Plan the new block sequence: kept source blocks verbatim, kept pieces of
    // straddling blocks (and any block a patch intersects) re-encoded.
    let mut plan: Vec<CompactBlock> = Vec::new();
    let mut table: Vec<(usize, usize, u16)> = Vec::new();
    let mut payload_out: Vec<u8> = Vec::new();
    let mut touched = Vec::new();
    let mut block_start = 0usize;
    for (index, block) in bundle.blocks.iter().enumerate() {
        super::cancelled()?;
        let block_end = block_start.checked_add(block.decoded).ok_or_else(|| invalid("UnityFS block extent overflow"))?;
        let mut pieces: Vec<(usize, usize)> = Vec::new();
        if block_end <= from || block_start >= to {
            pieces.push((0, block.decoded));
        } else {
            if block_start < from { pieces.push((0, from - block_start)); }
            if block_end > to { pieces.push((to - block_start, block.decoded)); }
        }
        for (local_start, local_end) in pieces {
            if local_start >= local_end { continue; }
            let old_piece_start = block_start + local_start;
            let new_piece_start = if old_piece_start < from { old_piece_start } else { old_piece_start - removed };
            let new_piece_end = new_piece_start + (local_end - local_start);
            let patched = ranges.iter().any(|(start, end, _)| *start < new_piece_end && new_piece_start < *end);
            let codec = (block.flags & 0x3f) as u32;
            if local_start == 0 && local_end == block.decoded && !patched {
                let source = b.get(block.offset..block.offset + block.encoded)
                    .ok_or_else(|| invalid("UnityFS source block outside file"))?;
                payload_out.extend_from_slice(source);
                table.push((block.decoded, block.encoded, block.flags));
                plan.push(CompactBlock::Copied { old: index });
                continue;
            }
            let source = b.get(block.offset..block.offset + block.encoded)
                .ok_or_else(|| invalid("UnityFS source block outside file"))?;
            let mut decoded_block = decode(source, block.decoded, codec)?;
            let mut kept = decoded_block.drain(local_start..local_end).collect::<Vec<u8>>();
            for (start, end, bytes) in &ranges {
                let first = (*start).max(new_piece_start);
                let last = (*end).min(new_piece_end);
                if first < last {
                    kept[first - new_piece_start..last - new_piece_start]
                        .copy_from_slice(&bytes[first - *start..last - *start]);
                }
            }
            let encoded = if codec == 0 { kept.clone() } else {
                compress(&kept, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?
            };
            super::cancelled()?;
            let roundtrip = decode(&encoded, kept.len(), codec).map_err(|error| invalid(&format!(
                "compacted UnityFS source block {index} failed round-trip verification: {error}")))?;
            if roundtrip != kept {
                return Err(invalid(&format!("compacted UnityFS source block {index} failed round-trip verification")));
            }
            touched.push((index, block.encoded, encoded.len()));
            table.push((kept.len(), encoded.len(), block.flags));
            payload_out.extend_from_slice(&encoded);
            plan.push(CompactBlock::Built { decoded: kept, old: index });
        }
        block_start = block_end;
    }
    if table.is_empty() || table.len() > 65536 { return Err(invalid("invalid compacted UnityFS block count")); }
    if table.iter().map(|(size, _, _)| *size).sum::<usize>() != new_decoded {
        return Err(invalid("compacted UnityFS decoded length mismatch"));
    }
    // Rebuild the metadata: the 16-byte hash placeholder is preserved verbatim,
    // the block/decoded/encoded/flag triples are rewritten, and the node table
    // keeps its order, flags and paths with the target shrunk and every later
    // node moved down by exactly the removed length.
    let mut info = bundle.info[..16].to_vec();
    info.extend_from_slice(&(table.len() as u32).to_be_bytes());
    for (size, encoded, flags) in &table {
        info.extend_from_slice(&(*size as u32).to_be_bytes());
        info.extend_from_slice(&(*encoded as u32).to_be_bytes());
        info.extend_from_slice(&flags.to_be_bytes());
    }
    info.extend_from_slice(&(bundle.nodes.len() as u32).to_be_bytes());
    for (index, node) in bundle.nodes.iter().enumerate() {
        let (offset, size) = if index == target {
            (node.offset, node.size - removed as u64)
        } else if node.offset >= to as u64 {
            (node.offset - removed as u64, node.size)
        } else { (node.offset, node.size) };
        info.extend_from_slice(&offset.to_be_bytes());
        info.extend_from_slice(&size.to_be_bytes());
        info.extend_from_slice(&node.flags.to_be_bytes());
        info.extend_from_slice(&node.path);
        info.push(0);
    }
    let codec = bundle.flags & 0x3f;
    let metadata = if codec == 0 { info.clone() } else {
        compress(&info, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?
    };
    if decode(&metadata, info.len(), codec)? != info { return Err(invalid("UnityFS metadata round-trip mismatch")); }
    let mut out = b[..bundle.header_end].to_vec();
    if bundle.version >= 7 { out.resize(align(out.len())?, 0); }
    if bundle.flags & 0x80 == 0 { out.extend_from_slice(&metadata); }
    if bundle.flags & 0x200 != 0 { out.resize(align(out.len())?, 0); }
    out.extend_from_slice(&payload_out);
    if bundle.flags & 0x80 != 0 { out.extend_from_slice(&metadata); }
    let length = out.len() as u64;
    out[bundle.fields..bundle.fields + 8].copy_from_slice(&length.to_be_bytes());
    out[bundle.fields + 8..bundle.fields + 12].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    // The block count can change (a block wholly inside the removed range
    // disappears), so unlike `rebuild_patches` the decoded metadata length must
    // be rewritten to the candidate's own info length, not kept from the source.
    out[bundle.fields + 12..bundle.fields + 16].copy_from_slice(&(info.len() as u32).to_be_bytes());
    let check = parse(&out).map_err(|error| invalid(&format!("compacted UnityFS candidate failed to parse: {error}")))?;
    if check.flags != bundle.flags || check.nodes.len() != bundle.nodes.len()
        || check.blocks.len() != table.len() || check.decoded != new_decoded as u64 {
        return Err(invalid("compacted UnityFS layout failed verification"));
    }
    for (index, (old, new)) in bundle.nodes.iter().zip(&check.nodes).enumerate() {
        if old.flags != new.flags || old.path != new.path { return Err(invalid("compacted UnityFS node entry changed")); }
        let expected = if index == target {
            (old.offset, old.size - removed as u64)
        } else if old.offset >= to as u64 {
            (old.offset - removed as u64, old.size)
        } else { (old.offset, old.size) };
        if (new.offset, new.size) != expected { return Err(invalid("compacted UnityFS node extent was not rewritten")); }
    }
    // Decode every new block and compare it against its exact expected content:
    // the source block for verbatim copies, the piece/patch splice otherwise.
    for ((entry, new), item) in table.iter().zip(&check.blocks).zip(&plan) {
        super::cancelled()?;
        let old_index = match item {
            CompactBlock::Copied { old } | CompactBlock::Built { old, .. } => *old,
        };
        let old = &bundle.blocks[old_index];
        if new.decoded != entry.0 || new.encoded != entry.1 || new.flags != entry.2 {
            return Err(invalid(&format!("compacted UnityFS block table mismatch in source block {old_index}")));
        }
        let expected = match item {
            CompactBlock::Copied { .. } => {
                if out[new.offset..new.offset + new.encoded] != b[old.offset..old.offset + old.encoded] {
                    return Err(invalid(&format!("compacted UnityFS copied source block {old_index} changed")));
                }
                decode(&b[old.offset..old.offset + old.encoded], old.decoded, (old.flags & 0x3f) as u32)?
            }
            CompactBlock::Built { decoded, .. } => decoded.clone(),
        };
        let actual = decode(&out[new.offset..new.offset + new.encoded], new.decoded, (new.flags & 0x3f) as u32)
            .map_err(|error| invalid(&format!("compacted UnityFS output block {old_index} cannot be decoded: {error}")))?;
        if expected != actual {
            return Err(invalid(&format!("compacted UnityFS content differs in output block {old_index}")));
        }
    }
    Ok((out, CompactionStats { removed, new_decoded: new_decoded as u64, touched }, bundle, check))
}


/// Read-only inventory of a UnityFS bundle: decode the blocks in memory and
/// summarize each SerializedFile node (version, object and class counts). Streams
/// (`*.resS`), resource files and opaque nodes are listed by kind. Nothing is
/// written and no writer is implied.
pub fn inventory(path: &Path) -> io::Result<()> {
    let bytes = read_source(path)?;
    let bundle = parse(&bytes)?;
    let mut decoded = Vec::new();
    for block in &bundle.blocks {
        super::cancelled()?;
        let source = bytes
            .get(block.offset..block.offset + block.encoded)
            .ok_or_else(|| invalid("UnityFS block outside file"))?;
        decoded.extend_from_slice(&decode(source, block.decoded, (block.flags & 0x3f) as u32)?);
    }
    if decoded.len() as u64 != bundle.decoded {
        return Err(invalid("UnityFS decoded length mismatch"));
    }
    println!(
        "UNITYFS_BUNDLE|{}|{}|{}|{}",
        bundle.version,
        bundle.flags,
        bundle.nodes.len(),
        bundle.decoded
    );
    let (mut serialized, mut textures, mut texture_bytes) = (0u64, 0u64, 0u64);
    for node in &bundle.nodes {
        let start = node.offset as usize;
        let end = start
            .checked_add(node.size as usize)
            .ok_or_else(|| invalid("UnityFS node extent overflow"))?;
        let slice = decoded.get(start..end).ok_or_else(|| invalid("UnityFS node outside decoded data"))?;
        let name = String::from_utf8_lossy(&node.path);
        if node.path.ends_with(b".resS") {
            println!("UNITYFS_NODE|{name}|{}|stream", node.size);
        } else if node.path.ends_with(b".resource") {
            println!("UNITYFS_NODE|{name}|{}|resource", node.size);
        } else {
            match crate::unity_serialized::bundle_summary(slice) {
                Ok(summary) => {
                    serialized += 1;
                    textures += summary.textures;
                    texture_bytes = texture_bytes.saturating_add(summary.texture_bytes);
                    println!(
                        "UNITYFS_NODE|{name}|{}|serialized|{}|{}|{}|{}|{}|{}|{}",
                        node.size,
                        summary.version,
                        summary.objects,
                        u8::from(summary.trees),
                        summary.textures,
                        summary.texture_bytes,
                        summary.sprites,
                        summary.audio
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(_) => println!("UNITYFS_NODE|{name}|{}|opaque", node.size),
            }
        }
    }
    super::cancelled()?;
    println!(
        "UNITYFS_TOTAL|{}|{}|{}|{}",
        bundle.nodes.len(),
        serialized,
        textures,
        texture_bytes
    );
    eprintln!("Read-only UnityFS bundle inventory: blocks are decoded in memory and SerializedFiles are summarized by metadata; object payloads stay opaque and no writer is implied.");
    Ok(())
}

fn read_source(path: &Path) -> io::Result<Vec<u8>> {
    read_source_limit(path, MAX_FILE)
}

fn read_source_limit(path: &Path, max_file: u64) -> io::Result<Vec<u8>> {
    super::cancelled()?;
    // Belt-and-suspenders symlink refusal. The flags here used to be the
    // hardcoded 0x20000, which is O_NOFOLLOW on x86_64 but not on aarch64, so
    // an aarch64 release runner followed the link and produced a candidate.
    // libc::O_NOFOLLOW is now correct per-architecture; this explicit check is a
    // second, flag-independent gate.
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("UnityFS source must not be a symlink"));
    }
    let mut f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    let before = f.metadata()?;
    if !before.is_file() || before.len() > max_file { return Err(invalid("UnityFS source exceeds its regular-file/size limit")); }
    let mut b = Vec::new();
    b.try_reserve_exact(before.len() as usize + 1)
        .map_err(|_| invalid("UnityFS source allocation limit exceeded"))?;
    let mut chunk = [0u8; 65536];
    while b.len() as u64 <= max_file {
        super::cancelled()?;
        let capacity = (max_file + 1 - b.len() as u64).min(chunk.len() as u64) as usize;
        let n = match f.read(&mut chunk[..capacity]) {
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => { super::cancelled()?; continue; }
            Err(error) => return Err(error),
        };
        if n == 0 { break; }
        b.try_reserve(n).map_err(|_| invalid("UnityFS source allocation limit exceeded"))?;
        b.extend_from_slice(&chunk[..n]);
    }
    super::cancelled()?;
    let after = f.metadata()?;
    if b.len() as u64 != before.len() || before.len() != after.len() || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec() || before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() {
        return Err(invalid("UnityFS source changed during read"));
    }
    Ok(b)
}

const MAX_TEXTURE_BUNDLE: u64 = 128 * 1024 * 1024;
const MAX_TEXTURE_NODES: usize = 4096;
const MAX_TEXTURE_REPORT: u64 = 8 * 1024 * 1024;

fn texture_bytes(t: &crate::unity_tree::Texture) -> io::Result<Option<u64>> {
    use crate::audit_io::Layout;
    let layout = match t.format {
        4 | 14 => Layout::pixels(4), // RGBA32 / BGRA32
        10 | 26 => Layout::bc(8), // DXT1 / BC4
        12 => Layout::bc(16),
        25 | 27 => Layout::bc(16), // BC7 / BC5
        _ => return Ok(None),
    };
    let mut total = 0u64;
    for level in 0..t.mips {
        total = total.checked_add(layout.bytes((t.width >> level).max(1), (t.height >> level).max(1))?)
            .ok_or_else(|| invalid("Unity texture payload size overflow"))?;
    }
    Ok(Some(total))
}

/// One texture object located by the shared selection rules of the detached
/// export and replace routes. `slice` is the already-decoded SerializedFile node
/// the object lives in; inline storage offsets are already relative to that node
/// (the object start is included), while `fields`' own spans stay object-relative.
/// The helper only locates bytes: it never decodes pixels, follows host paths or
/// writes.
#[cfg(any(test, feature = "development-audits"))]
struct LocatedTexture {
    node_offset: u64,
    slice: Vec<u8>,
    fields: crate::unity_tree::TextureFields,
    record: crate::unity_serialized::TextureObjectRecord,
    format: image_dds::ImageFormat,
    expected: u64,
    version: u32,
    big: bool,
    storage: LocatedStorage,
}

#[cfg(any(test, feature = "development-audits"))]
enum LocatedStorage {
    Inline { span_start: usize, payload_start: usize, payload_len: usize },
    Streamed { target: usize, offset: u64, size: u64 },
}

/// The single Texture2D selection used by both `unityfs-texture-export` and
/// `unityfs-texture-replace`, so their acceptance rules cannot drift. It refuses
/// ambiguous identities, overlapping nodes, streamed extents with a known alias
/// and unknown shapes exactly as the export route always has.
#[cfg(any(test, feature = "development-audits"))]
fn locate_texture(node_index: usize, path_id: u64, bundle: &Bundle,
    decoder: &mut NodeDecoder<'_>) -> io::Result<LocatedTexture> {
    locate_texture_with_slice(node_index, path_id, bundle, decoder, None)
}

/// The same selection with an already-decoded SerializedFile node supplied by
/// the caller. The apply route inventories each node once and cannot afford a
/// fresh decoder read per texture; the supplied slice must be exactly the
/// selected node's decoded bytes or the call fails closed.
#[cfg(any(test, feature = "development-audits"))]
fn locate_texture_with_slice(node_index: usize, path_id: u64, bundle: &Bundle,
    decoder: &mut NodeDecoder<'_>, supplied_slice: Option<&[u8]>) -> io::Result<LocatedTexture> {
    if bundle.decoded > MAX_TEXTURE_BUNDLE || bundle.nodes.len() > MAX_TEXTURE_NODES {
        return Err(invalid("Unity texture export exceeds its 128 MiB/4096-node work limit"));
    }
    let mut node_extents = Vec::new();
    for node in &bundle.nodes {
        if node.size != 0 { node_extents.push((node.offset, node.offset + node.size)); }
    }
    node_extents.sort_unstable();
    if node_extents.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid("Unity texture export refuses aliased/overlapping bundle nodes"));
    }
    let mut names = std::collections::BTreeMap::<Vec<u8>, usize>::new();
    for (index, node) in bundle.nodes.iter().enumerate() {
        if names.insert(node.path.clone(), index).is_some() {
            return Err(invalid("Unity texture export refuses ambiguous duplicate node names"));
        }
    }
    let Some(node) = bundle.nodes.get(node_index) else {
        return Err(invalid("Unity bundle node index is out of range"));
    };
    // Directory flag 4 (1 << 2) designates SerializedFiles in this subset.
    if node.flags != 4 { return Err(invalid("selected Unity node is not a SerializedFile (flags != 4)")); }
    if node.size > 64 * 1024 * 1024 { return Err(invalid("selected Unity SerializedFile exceeds the 64 MiB node limit")); }
    let slice = match supplied_slice {
        Some(slice) if slice.len() as u64 == node.size => slice.to_vec(),
        Some(_) => return Err(invalid("supplied Unity SerializedFile slice does not match its node extent")),
        None => decoder.read(node.offset, node.size)?,
    };
    if !crate::unity_serialized::plausible(&slice) {
        return Err(invalid("selected Unity node is not a supported SerializedFile header"));
    }
    let mut metadata_budget = 32u64 * 1024 * 1024;
    let mut object_budget = 64u64 * 1024 * 1024;
    let mut report_budget = 10000u64;
    let mut inventory = crate::unity_serialized::bundle_texture_inventory(
        &slice, &mut metadata_budget, &mut object_budget, &mut report_budget)?;
    let matches = inventory.details.iter().filter(|(id, _)| *id == path_id).count();
    if matches == 0 {
        let reason = inventory.opaque.iter().find(|(id, _)| *id == path_id)
            .map(|(_, status)| *status)
            .unwrap_or("NOT_A_RETAINED_METADATA_TEXTURE_OBJECT");
        return Err(invalid(&format!("Unity path ID {path_id} has no supported texture ({reason})")));
    }
    if matches > 1 {
        return Err(invalid("Unity path ID matches more than one retained texture object"));
    }
    let position = inventory.details.iter().position(|(id, _)| *id == path_id).unwrap();
    let (_, fields) = inventory.details.swap_remove(position);
    let record = inventory.object_records.iter().find(|record| record.id == path_id)
        .ok_or_else(|| invalid("Unity texture object record was not retained"))?.clone();
    let texture = &fields.texture;
    if fields.image_count != Some(1) || fields.dimension != Some(2) {
        return Err(invalid("Unity texture export requires m_ImageCount 1 and m_TextureDimension 2"));
    }
    let format = match texture.format {
        4 => image_dds::ImageFormat::Rgba8Unorm,
        14 => image_dds::ImageFormat::Bgra8Unorm,
        10 => image_dds::ImageFormat::BC1RgbaUnorm,
        12 => image_dds::ImageFormat::BC3RgbaUnorm,
        26 => image_dds::ImageFormat::BC4RUnorm,
        25 => image_dds::ImageFormat::BC7RgbaUnorm,
        27 => image_dds::ImageFormat::BC5RgUnorm,
        other => return Err(invalid(&format!("Unity texture format {other} is outside the export subset"))),
    };
    let expected = match texture_bytes(texture)? {
        Some(expected) if expected > 0 => expected,
        _ => return Err(invalid("Unity texture codec/mip shape is outside the export subset")),
    };
    if texture.inline_bytes != 0 && texture.stream_size != 0 {
        return Err(invalid("Unity texture declares both inline and streamed storage"));
    }
    let storage = if texture.inline_bytes != 0 {
        if texture.inline_bytes as u64 != expected {
            return Err(invalid("Unity inline payload length disagrees with its known mip shape"));
        }
        let start = record.start as usize;
        let end = start.checked_add(record.size as usize).ok_or_else(|| invalid("Unity object extent overflow"))?;
        let object = slice.get(start..end)
            .ok_or_else(|| invalid("Unity texture object is outside its SerializedFile node"))?;
        // Reuse the module's borrow-checked span reader: it refuses a missing or
        // unexpected span, a declared length that disagrees with the texture and
        // an out-of-bounds extent. Only after it succeeds are the node-relative
        // offsets derived for the replace route to splice (the length prefix is
        // the four bytes immediately before the payload).
        let payload = crate::unity_tree::inline_image(&fields, object)?;
        let span = fields.spans.get("image data").or_else(|| fields.spans.get("image data/Array"))
            .ok_or_else(|| invalid("missing Unity inline image span"))?;
        let (payload_start, payload_len) = span.payload
            .ok_or_else(|| invalid("Unity inline image span has no payload"))?;
        if payload.len() != payload_len || span.start.checked_add(4) != Some(payload_start)
            || payload_start.checked_add(payload_len).is_none_or(|end| end > object.len()) {
            return Err(invalid("Unity inline image span layout cannot be proven"));
        }
        LocatedStorage::Inline { span_start: start + span.start,
            payload_start: start + payload_start, payload_len }
    } else if texture.stream_size != 0 {
        if texture.stream_size != expected {
            return Err(invalid("Unity streamed extent length disagrees with its known mip shape"));
        }
        let name = stream_node(&node.path, &texture.stream_path)
            .ok_or_else(|| invalid("Unity stream path is outside the supported same-bundle namespace"))?;
        let Some(&target) = names.get(name) else {
            return Err(invalid("Unity stream target node is not present in this bundle"));
        };
        if target == node_index { return Err(invalid("Unity stream target is the SerializedFile itself")); }
        let target_node = &bundle.nodes[target];
        if target_node.flags != 0 { return Err(invalid("Unity stream target is not a resource node")); }
        let end = texture.stream_offset.checked_add(texture.stream_size)
            .ok_or_else(|| invalid("Unity stream extent overflow"))?;
        if end > target_node.size { return Err(invalid("Unity stream extent is outside its target node")); }
        // Require a unique claim: no other retained texture object may declare an
        // overlapping range into the same target node. Stream ownership stays
        // unproven even then; this only rejects known aliases.
        for (other_id, other) in inventory.details.iter() {
            if *other_id == path_id || other.texture.stream_size == 0 { continue; }
            let Some(other_name) = stream_node(&node.path, &other.texture.stream_path) else { continue; };
            if names.get(other_name) != Some(&target) { continue; }
            let other_end = other.texture.stream_offset.saturating_add(other.texture.stream_size);
            if other.texture.stream_offset < end && texture.stream_offset < other_end {
                return Err(invalid("Unity stream extent aliases another retained texture object"));
            }
        }
        LocatedStorage::Streamed { target, offset: texture.stream_offset, size: texture.stream_size }
    } else {
        return Err(invalid("Unity texture has no inline or streamed payload"));
    };
    Ok(LocatedTexture { node_offset: node.offset, slice, fields, record, format, expected,
        version: inventory.version, big: inventory.big, storage })
}

#[cfg(any(test, feature = "development-audits"))]
fn read_u32(bytes: &[u8], offset: usize, big: bool) -> io::Result<u32> {
    let end = offset.checked_add(4).ok_or_else(|| invalid("Unity field offset overflow"))?;
    let field = bytes.get(offset..end).ok_or_else(|| invalid("truncated Unity field"))?;
    Ok(if big { u32::from_be_bytes(field.try_into().unwrap()) } else { u32::from_le_bytes(field.try_into().unwrap()) })
}

#[cfg(any(test, feature = "development-audits"))]
fn read_u64(bytes: &[u8], offset: usize, big: bool) -> io::Result<u64> {
    let end = offset.checked_add(8).ok_or_else(|| invalid("Unity field offset overflow"))?;
    let field = bytes.get(offset..end).ok_or_else(|| invalid("truncated Unity field"))?;
    Ok(if big { u64::from_be_bytes(field.try_into().unwrap()) } else { u64::from_le_bytes(field.try_into().unwrap()) })
}

/// Development-only detached export of one inline Unity SerializedFile
/// Texture2D into a standalone DDS. Selection is explicit: `node_index` is the
/// bundle directory node index and `path_id` the original 64-bit object path ID.
/// Only an ordinary 2D image (`m_ImageCount` 1, `m_TextureDimension` 2) with
/// inline storage and a known codec/mip shape is accepted. Streamed textures,
/// unknown formats, ambiguous identities and non-SerializedFile nodes are
/// refused rather than guessed. The source is never modified, and no bundle,
/// object table, atlas reference or catalogue/CRC integrity is rebuilt or
/// claimed. The inline payload is copied verbatim; no decode/re-encode occurs.
#[cfg(any(test, feature = "development-audits"))]
pub fn texture_export(node_index: usize, path_id: u64, input: &Path, output: &Path) -> io::Result<()> {
    use crate::audit_io::Source;
    use image_dds::Surface;
    match std::fs::symlink_metadata(output) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Unity texture export destination exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut source = Source::open(input, MAX_TEXTURE_BUNDLE, MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024)?;
    let bytes = source.read_at(0, source.len() as usize)?;
    let bundle = parse(&bytes)?;
    let mut decoder = NodeDecoder::new(&bytes, &bundle);
    let located = locate_texture(node_index, path_id, &bundle, &mut decoder)?;
    let texture = &located.fields.texture;
    let format = located.format;
    let (payload, origin) = match located.storage {
        LocatedStorage::Inline { payload_start, payload_len, .. } => (
            std::borrow::Cow::Borrowed(&located.slice[payload_start..payload_start + payload_len]),
            "INLINE"),
        LocatedStorage::Streamed { target, offset, size } => {
            let target_node = &bundle.nodes[target];
            let bytes = decoder.read(target_node.offset + offset, size)?;
            if bytes.len() as u64 != located.expected {
                return Err(invalid("Unity streamed payload read a different length"));
            }
            (std::borrow::Cow::Owned(bytes), "STREAM_BOUNDS_ONLY_OWNERSHIP_UNPROVEN")
        }
    };
    let surface = Surface { width: texture.width, height: texture.height, depth: 1,
        layers: 1, mipmaps: texture.mips, image_format: format, data: payload.as_ref() };
    let candidate = crate::texture::build_dds(&surface)?;
    // Re-parse the immutable candidate with the same DDS parser, then decode its
    // base mip. This checks internal self-consistency only, not independent
    // parser agreement, atlas references or runtime compatibility.
    let check = crate::texture::parse(&candidate)?;
    if check.width != texture.width || check.height != texture.height || check.mipmaps != texture.mips
        || check.format != format || &candidate[check.data_start..] != payload.as_ref() {
        return Err(invalid("Unity DDS export failed its self-check"));
    }
    {
        let decoded = Surface { width: texture.width, height: texture.height, depth: 1,
            layers: 1, mipmaps: texture.mips, image_format: format, data: &candidate[check.data_start..] }
            .decode_rgba8()
            .map_err(|error| invalid(&format!("Unity exported texture decode failed: {error}")))?;
        if decoded.width != texture.width || decoded.height != texture.height
            || decoded.depth != 1 || decoded.layers != 1 {
            return Err(invalid("Unity exported texture decoded to an unexpected shape"));
        }
    }
    super::detached_export::publish(output, &candidate, || source.unchanged())?;
    println!("UNITYFS_TEXTURE_EXPORT|{node_index}|{path_id}|{}|{}|{}|{}|{}|{}|{}|{origin}|EXPORTED",
        texture.width, texture.height, texture.format,
        crate::unity_tree::format_name(texture.format), texture.mips,
        payload.len(), candidate.len());
    eprintln!("Experimental detached Unity texture export: source unchanged; verbatim payload only. Streamed ranges are bounds-checked and non-aliasing but stream ownership is unproven; atlas/catalogue/CRC rebuild and runtime compatibility are unverified.");
    Ok(())
}

/// Full mip-chain level count for one declared dimension pair: every level down
/// to 1x1, which is exactly the bound the tree inspector already enforces for a
/// retained texture. A shrink payload must be the complete chain of its new
/// shape, so its byte length is a function of (width, height, format) alone.
#[cfg(any(test, feature = "development-audits"))]
fn full_mip_levels(width: u32, height: u32) -> u32 {
    32 - width.max(height).leading_zeros()
}

/// Known-codec chain length for one declared shape, from the same layout table
/// the selection helper uses. Unknown codecs and empty chains fail closed.
#[cfg(any(test, feature = "development-audits"))]
fn texture_shape_bytes(format: u32, width: u32, height: u32, mips: u32) -> io::Result<u64> {
    let texture = crate::unity_tree::Texture { width, height, format, mips,
        inline_bytes: 0, stream_offset: 0, stream_size: 0, stream_path: String::new() };
    match texture_bytes(&texture)? {
        Some(bytes) if bytes > 0 => Ok(bytes),
        _ => Err(invalid("Unity texture codec/shape is outside the supported subset")),
    }
}

/// Infer and validate the single downscale implied by a payload length. Both
/// dimensions are halved the same number of times (never upscaled), the payload
/// must be the complete 1x1-terminated chain of the result, and the result must
/// pass the repository texture policy (short edge) plus the 4x4 block floor on
/// every axis that is already at least 4. A chain length strictly decreases per
/// halving, so at most one shape can match and nothing is ever guessed. An empty
/// result means "not a downscale payload"; a refused one explains itself.
#[cfg(any(test, feature = "development-audits"))]
fn shrink_shape(texture: &crate::unity_tree::Texture, payload_len: usize) -> io::Result<Option<(u32, u32, u32)>> {
    let (width, height) = (texture.width, texture.height);
    let target = payload_len as u64;
    let mut found = None;
    for level in 1..32u32 {
        let (new_width, new_height) = (width >> level, height >> level);
        if new_width == 0 || new_height == 0 { break; }
        let mips = full_mip_levels(new_width, new_height);
        if texture_shape_bytes(texture.format, new_width, new_height, mips)? == target {
            if found.is_some() {
                return Err(invalid("Unity shrink payload length matches more than one downscale"));
            }
            found = Some((new_width, new_height, mips));
        }
    }
    let Some((new_width, new_height, mips)) = found else { return Ok(None) };
    if crate::texture_policy::output_too_thin(new_width, new_height) {
        return Err(invalid(&format!(
            "Unity texture downscale to {new_width}x{new_height} is refused by the repository texture policy (short edge below {})",
            crate::texture_policy::MIN_SHORT_EDGE)));
    }
    if (width >= 4 && new_width < 4) || (height >= 4 && new_height < 4) {
        return Err(invalid("Unity texture downscale below the 4x4 block floor is not supported"));
    }
    Ok(Some((new_width, new_height, mips)))
}

/// Prove a supplied shrink payload is a self-consistent mip chain for the new
/// declared shape before any byte is written, with the same DDS wrapper and
/// parser the export route uses, then decode every level. This proves chain
/// geometry and per-level decodability in the source format; for BC and
/// uncompressed codecs any bytes of the right length decode, so pixel
/// correctness and visual equivalence are not proven.
#[cfg(any(test, feature = "development-audits"))]
fn prove_shrink_chain(format: image_dds::ImageFormat, width: u32, height: u32,
    mips: u32, payload: &[u8]) -> io::Result<()> {
    use image_dds::Surface;
    let surface = Surface { width, height, depth: 1, layers: 1, mipmaps: mips,
        image_format: format, data: payload };
    let candidate = crate::texture::build_dds(&surface)?;
    let check = crate::texture::parse(&candidate)?;
    if check.width != width || check.height != height || check.mipmaps != mips
        || check.format != format || &candidate[check.data_start..] != payload {
        return Err(invalid("Unity shrink payload failed its mip-chain self-check"));
    }
    let decoded = Surface { width, height, depth: 1, layers: 1, mipmaps: mips,
        image_format: format, data: &candidate[check.data_start..] }
        .decode_rgba8()
        .map_err(|error| invalid(&format!("Unity shrink payload decode failed: {error}")))?;
    if decoded.width != width || decoded.height != height || decoded.depth != 1 || decoded.layers != 1 {
        return Err(invalid("Unity shrink payload decoded to an unexpected shape"));
    }
    Ok(())
}

/// Fresh shared-budget inventory for one already-decoded SerializedFile node.
#[cfg(any(test, feature = "development-audits"))]
fn node_texture_inventory(node: &[u8]) -> io::Result<crate::unity_serialized::BundleTextureInventory> {
    let mut metadata_budget = 32u64 * 1024 * 1024;
    let mut object_budget = 64u64 * 1024 * 1024;
    let mut report_budget = 10000u64;
    crate::unity_serialized::bundle_texture_inventory(node, &mut metadata_budget, &mut object_budget, &mut report_budget)
}

/// One replacement byte range inside the decoded SerializedFile node, in
/// node-relative coordinates. `bytes.len() == end - start`; patches returned
/// for one texture never overlap each other.
#[cfg(any(test, feature = "development-audits"))]
struct NodePatch {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

/// Refuse any set of same-node patches that overlap instead of guessing which
/// replacement wins. Adjacent ranges are allowed.
#[cfg(any(test, feature = "development-audits"))]
fn check_patches_disjoint(patches: &[NodePatch], context: &str) -> io::Result<()> {
    for pair in patches.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(invalid(&format!("Unity texture {context} patches overlap")));
        }
    }
    Ok(())
}

/// Write one proven 4-byte object field as a node-relative patch, requiring a
/// primitive span that holds exactly the value the tree walk parsed. The
/// expected value is checked through the same span before any byte is produced.
#[cfg(any(test, feature = "development-audits"))]
fn field_patch(located: &LocatedTexture, path: &str, value: u32, expected: u32) -> io::Result<NodePatch> {
    let span = located.fields.spans.get(path)
        .filter(|span| span.kind == "PRIMITIVE_BITS" && span.end == span.start + 4)
        .ok_or_else(|| invalid(&format!("Unity texture apply requires the {path} field span")))?;
    let object_start = located.record.start as usize;
    let at = object_start.checked_add(span.start).ok_or_else(|| invalid("Unity texture field offset overflow"))?;
    if read_u32(&located.slice, at, located.big)? != expected {
        return Err(invalid(&format!("Unity texture {path} span does not hold the parsed value")));
    }
    let bytes = if located.big { value.to_be_bytes().to_vec() } else { value.to_le_bytes().to_vec() };
    Ok(NodePatch { start: at, end: at + 4, bytes })
}

/// Every byte an inline simplify changes, in node-relative coordinates: the
/// image-data length prefix, the declared shape fields (and m_CompleteImageSize
/// when the schema has it), the new payload and the zero tail that keeps the
/// fields after the payload reading their parsed values. The same rules the
/// detached replace route always applied; nothing is written by this helper.
#[cfg(any(test, feature = "development-audits"))]
fn inline_shrink_patches(located: &LocatedTexture, new_payload: &[u8],
    new_width: u32, new_height: u32, new_mips: u32) -> io::Result<Vec<NodePatch>> {
    let old_texture = &located.fields.texture;
    let LocatedStorage::Inline { span_start, payload_start, payload_len } = located.storage else {
        return Err(invalid("Unity inline shrink requires inline storage"));
    };
    let new_len = new_payload.len();
    if new_len > payload_len {
        return Err(invalid("Unity inline shrink payload exceeds the current inline extent"));
    }
    let shrink = new_len != payload_len;
    if read_u32(&located.slice, span_start, located.big)? as usize != payload_len {
        return Err(invalid("Unity inline image length prefix cannot be proven"));
    }
    let object_start = located.record.start as usize;
    let object_end = object_start.checked_add(located.record.size as usize)
        .ok_or_else(|| invalid("Unity object extent overflow"))?;
    let payload_extent_end = payload_start.checked_add(payload_len)
        .ok_or_else(|| invalid("Unity inline payload extent overflow"))?;
    if payload_extent_end > object_end || object_end > located.slice.len() {
        return Err(invalid("Unity inline payload extent cannot be proven"));
    }
    // The fields that follow the payload are read sequentially from the zero
    // tail after a shrink, so every byte after the old extent must already be
    // zero: that is what keeps their parsed values identical.
    if shrink && located.slice[payload_extent_end..object_end].iter().any(|byte| *byte != 0) {
        return Err(invalid("Unity texture shrink requires every byte after the current inline payload to be zero; the fields that follow it would otherwise change"));
    }
    let length_bytes = if located.big { (new_len as u32).to_be_bytes().to_vec() } else { (new_len as u32).to_le_bytes().to_vec() };
    let mut patches = vec![
        NodePatch { start: span_start, end: span_start + 4, bytes: length_bytes },
        field_patch(located, "m_Width", new_width, old_texture.width)?,
        field_patch(located, "m_Height", new_height, old_texture.height)?,
        field_patch(located, "m_MipCount", new_mips, old_texture.mips)?,
    ];
    match located.fields.spans.get("m_CompleteImageSize") {
        Some(_) => patches.push(field_patch(located, "m_CompleteImageSize", new_len as u32, payload_len as u32)?),
        None if shrink => return Err(invalid("Unity texture shrink requires the m_CompleteImageSize field span")),
        None => (),
    }
    patches.push(NodePatch { start: payload_start, end: payload_start + new_len, bytes: new_payload.to_vec() });
    if shrink {
        patches.push(NodePatch { start: payload_start + new_len, end: payload_extent_end,
            bytes: vec![0u8; payload_len - new_len] });
    }
    patches.sort_unstable_by_key(|patch| patch.start);
    check_patches_disjoint(&patches, "inline shrink")?;
    Ok(patches)
}

/// The declared-shape fields a streamed shrink changes in its owning
/// SerializedFile node, in node-relative coordinates: width, height, mip count,
/// m_StreamData/size and m_CompleteImageSize. The stream bytes themselves are
/// patched or compacted separately. The route always required a proven
/// zero-length inline image span, so no inline payload can be silently ignored.
#[cfg(any(test, feature = "development-audits"))]
fn streamed_shrink_patches(located: &LocatedTexture, new_len: usize,
    new_width: u32, new_height: u32, new_mips: u32) -> io::Result<Vec<NodePatch>> {
    let old_texture = &located.fields.texture;
    let object_start = located.record.start as usize;
    // Streamed textures carry zero inline bytes; the image-data length prefix
    // must be a proven zero-length field so no inline payload is silently
    // ignored, and the locate route already refused a texture that declares both.
    let span = located.fields.spans.get("image data")
        .or_else(|| located.fields.spans.get("image data/Array"))
        .ok_or_else(|| invalid("Unity streamed texture is missing its image data span"))?;
    if span.payload != Some((span.start + 4, 0))
        || read_u32(&located.slice, object_start + span.start, located.big)? != 0 {
        return Err(invalid("Unity streamed texture has nonzero inline image bytes"));
    }
    let old_stream_size = u32::try_from(old_texture.stream_size)
        .map_err(|_| invalid("Unity stream extent exceeds its 32-bit size field"))?;
    let mut patches = vec![
        field_patch(located, "m_Width", new_width, old_texture.width)?,
        field_patch(located, "m_Height", new_height, old_texture.height)?,
        field_patch(located, "m_MipCount", new_mips, old_texture.mips)?,
        field_patch(located, "m_StreamData/size", new_len as u32, old_stream_size)?,
    ];
    match located.fields.spans.get("m_CompleteImageSize") {
        Some(_) => patches.push(field_patch(located, "m_CompleteImageSize", new_len as u32, old_stream_size)?),
        None => return Err(invalid("Unity streamed texture shrink requires the m_CompleteImageSize field span")),
    }
    patches.sort_unstable_by_key(|patch| patch.start);
    check_patches_disjoint(&patches, "streamed shrink")?;
    Ok(patches)
}


/// Development-only detached rewrite of one Unity SerializedFile Texture2D
/// payload, published as a NEW bundle file. Selection, identity and storage
/// rules are exactly the export route's. An inline texture accepts two payload
/// shapes and both keep every object offset in the SerializedFile table
/// byte-stable:
///   * the exact known mip-chain length for the unchanged width/height/mip
///     count/format (a same-shape chain has a single length), or
///   * a strictly shorter complete 1x1-terminated mip chain for dimensions that
///     are the current ones halved one or more times (never upscaled), accepted
///     by the repository texture policy (short edge >= 64) and never pushed
///     below the 4x4 block floor on an axis that already reaches 4.
/// A chain length determines its shape uniquely, so a length that matches
/// neither rule is refused rather than guessed. Because the object's serialized
/// size is kept exactly as before, the bytes between the new payload end and the
/// old payload end are zero-filled ("zero tail"): every byte after the new
/// payload in the object extent must already be zero, so the fields that follow
/// the payload keep their parsed values although Unity reads them sequentially,
/// not by absolute offset. Such an object no longer consumes its declared extent
/// exactly; the in-house reader accepts exactly one trailing run of zero bytes
/// for that reason. The zero tail compresses away only when the covering blocks
/// are really compressed: a stored-codec bundle rebuilds to its original size
/// and reports NO_GAIN. Only a strictly smaller container is published; NO_GAIN
/// publishes nothing.
///
/// A streamed (`*.resS`) texture is delegated to `replace_streamed_texture`: it
/// accepts only a strictly shorter halved-mip chain and physically compacts the
/// uniquely-claimed stream extent (removing the freed tail and shifting every
/// proven later stream offset) when every reference can be proven, falling back
/// to the prior zero-padded in-place rewrite otherwise.
///
/// The image-data length prefix, width, height, mip count and (when the schema
/// has the field) m_CompleteImageSize are patched through the spans the tree
/// walk captured, each of which must hold exactly the value the walk parsed. The
/// supplied bytes are spliced verbatim: no pixel or codec re-encode happens.
/// Only blocks that intersect the patched range are re-encoded with their source
/// codec, every other block keeps its source bytes. A nonzero uncompressed-data
/// hash field is refused because its algorithm is unproven; an all-zero field is
/// preserved byte-for-byte. No CRC, CAB name, Addressables catalogue or atlas
/// reference is rebuilt, the zero-padded extent is not proven to be accepted by
/// the Unity runtime, and the artifact is not runtime validated. The source is
/// never modified.
#[cfg(any(test, feature = "development-audits"))]
pub fn texture_replace(node_index: usize, path_id: u64, payload_path: &Path, input: &Path, output: &Path) -> io::Result<()> {
    use crate::audit_io::Source;
    match std::fs::symlink_metadata(output) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Unity texture replace destination exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut source = Source::open(input, MAX_TEXTURE_BUNDLE, MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024)?;
    let bytes = source.read_at(0, source.len() as usize)?;
    let bundle = parse(&bytes)?;
    let mut decoder = NodeDecoder::new(&bytes, &bundle);
    let located = locate_texture(node_index, path_id, &bundle, &mut decoder)?;
    // The 16-byte info-section field is the uncompressed-data hash. Its algorithm
    // is not proven here, so a nonzero value could not be recomputed after the
    // payload changes; that source is refused rather than published stale. The
    // sampled real Addressables bundles store all zeroes, a placeholder this
    // build preserves byte-for-byte exactly like the recompression route.
    if bundle.info[..16].iter().any(|byte| *byte != 0) {
        return Err(invalid("UnityFS source carries a nonzero uncompressed-data hash that cannot be recomputed safely"));
    }
    // Prove the SerializedFile object-table layout from the parsed version and
    // table extents before trusting any byte offset:
    // entry = id(8) + offset(4/8) + size(4) + type_index(4).
    let offset_width = if located.version == 22 { 8u64 } else { 4u64 };
    if located.record.table_end != located.record.table_start + 8 + offset_width + 8 {
        return Err(invalid("Unity SerializedFile object table layout cannot be proven"));
    }
    let size_field = located.record.table_start + 8 + offset_width;
    let stored_size = read_u32(&located.slice, size_field as usize, located.big)?;
    if stored_size as u64 != located.record.size {
        return Err(invalid("Unity SerializedFile object table size cannot be proven"));
    }
    // Streamed storage takes the two-node route: the target object's fields and
    // the `.resS` extent are patched or compacted in one verified rebuild.
    if matches!(&located.storage, LocatedStorage::Streamed { .. }) {
        return replace_streamed_texture(&bytes, &bundle, &located, node_index, path_id,
            payload_path, output, &source);
    }
    let payload_len = match &located.storage {
        LocatedStorage::Inline { payload_len, .. } => *payload_len,
        _ => return Err(invalid("Unity texture replace requires inline or uniquely-claimed streamed storage")),
    };
    // Two payload shapes are accepted, and neither moves an object offset:
    //  * the exact known mip-chain length for the unchanged width/height/
    //    mip-count/format shape (a chain has a single length per shape), and
    //  * the complete 1x1-terminated chain of a strictly smaller shape obtained
    //    by halving both dimensions one or more times (never upscaled). The
    //    payload length determines that shape uniquely.
    let mut payload_source = Source::open(payload_path, MAX_TEXTURE_BUNDLE, MAX_TEXTURE_BUNDLE + 16)?;
    let new_len = payload_source.len();
    if new_len > u32::MAX as u64 {
        return Err(invalid("Unity replacement payload exceeds the 4 GiB length prefix"));
    }
    let new_len = new_len as usize;
    let new_payload = payload_source.read_at(0, new_len)?;
    let old_texture = &located.fields.texture;
    let shrink = new_len != payload_len;
    let (new_width, new_height, new_mips) = if !shrink {
        (old_texture.width, old_texture.height, old_texture.mips)
    } else if new_len > payload_len {
        return Err(invalid(&format!(
            "Unity replacement payload is {new_len} bytes, larger than the current {payload_len}-byte inline extent; payload size increases are refused")));
    } else {
        match shrink_shape(old_texture, new_len)? {
            Some(shape) => shape,
            None => return Err(invalid(&format!(
                "Unity replacement payload must be exactly {payload_len} bytes for the unchanged {}x{} format {} {} mip chain, or the complete 1x1-terminated chain of a downscale of that shape (both dimensions halved one or more times, short edge at least {}); {new_len} bytes given",
                old_texture.width, old_texture.height, old_texture.format, old_texture.mips,
                crate::texture_policy::MIN_SHORT_EDGE))),
        }
    };
    if shrink {
        prove_shrink_chain(located.format, new_width, new_height, new_mips, &new_payload)?;
    }
    // The serialized size is kept exactly as it was, so the object table entry
    // needs no rewrite and no later object offset can move.
    let patches_for_object = inline_shrink_patches(&located, &new_payload, new_width, new_height, new_mips)?;
    let mut expected_node = located.slice.clone();
    for patch in &patches_for_object {
        expected_node[patch.start..patch.end].copy_from_slice(&patch.bytes);
    }
    let patch_lo = patches_for_object.iter().map(|patch| patch.start).min()
        .ok_or_else(|| invalid("Unity inline shrink produced no patches"))?;
    let patch_hi = patches_for_object.iter().map(|patch| patch.end).max()
        .ok_or_else(|| invalid("Unity inline shrink produced no patches"))?;
    let (candidate, _changed, _) = rebuild_patched(&bytes,
        Some((located.node_offset as usize + patch_lo, &expected_node[patch_lo..patch_hi])))?;
    // (1) The candidate must re-parse as a UnityFS bundle with the same layout.
    let check_bundle = parse(&candidate)?;
    if check_bundle.flags != bundle.flags || check_bundle.nodes.len() != bundle.nodes.len()
        || check_bundle.blocks.len() != bundle.blocks.len() || check_bundle.decoded != bundle.decoded {
        return Err(invalid("Unity texture replace changed the bundle layout"));
    }
    // (2) Every retained object entry must be byte-stable and the target must
    // re-parse with the new declared shape. The SerializedFile object table, all
    // other objects in the same node and every other bundle node are compared.
    let mut check_decoder = NodeDecoder::new(&candidate, &check_bundle);
    let check_node = &check_bundle.nodes[node_index];
    if check_node.offset != bundle.nodes[node_index].offset || check_node.size != bundle.nodes[node_index].size
        || check_node.flags != bundle.nodes[node_index].flags || check_node.path != bundle.nodes[node_index].path {
        return Err(invalid("Unity texture replace changed the bundle node table"));
    }
    let check_slice = check_decoder.read(check_node.offset, check_node.size)?;
    let before_inventory = node_texture_inventory(&located.slice)?;
    let after_inventory = node_texture_inventory(&check_slice)?;
    if after_inventory.version != before_inventory.version || after_inventory.big != before_inventory.big
        || after_inventory.textures != before_inventory.textures
        || after_inventory.object_records.len() != before_inventory.object_records.len() {
        return Err(invalid("Unity texture replace changed the SerializedFile object table"));
    }
    for (before, after) in before_inventory.object_records.iter().zip(&after_inventory.object_records) {
        if before.id != after.id || before.start != after.start || before.size != after.size
            || before.type_index != after.type_index || before.table_start != after.table_start
            || before.table_end != after.table_end {
            return Err(invalid("Unity texture replace changed a SerializedFile object entry"));
        }
    }
    let (_, declared) = after_inventory.details.iter().find(|(id, _)| *id == path_id)
        .ok_or_else(|| invalid("Unity texture replace target no longer re-parses as a supported texture"))?;
    if declared.texture.width != new_width || declared.texture.height != new_height
        || declared.texture.mips != new_mips || declared.texture.format != old_texture.format
        || declared.texture.inline_bytes != new_len
        || declared.image_count != Some(1) || declared.dimension != Some(2) {
        return Err(invalid("Unity texture replace target did not re-parse with the new declared shape"));
    }
    // (3) The payload must re-extract identically through the same selection, and
    // the whole replaced node must equal the spliced expectation.
    let check = locate_texture(node_index, path_id, &check_bundle, &mut check_decoder)?;
    if check.version != located.version || check.big != located.big
        || check.expected != new_len as u64
        || check.fields.texture.width != new_width
        || check.fields.texture.height != new_height
        || check.fields.texture.format != old_texture.format
        || check.fields.texture.mips != new_mips
        || check.fields.texture.inline_bytes != new_len {
        return Err(invalid("Unity texture replace changed the located texture identity"));
    }
    let LocatedStorage::Inline { payload_start: checked_start, payload_len: checked_len, .. } = check.storage else {
        return Err(invalid("Unity texture replace did not produce inline storage"));
    };
    if checked_len != new_len
        || &check.slice[checked_start..checked_start + checked_len] != &new_payload[..]
        || check.slice != expected_node {
        return Err(invalid("Unity texture replace payload failed re-extraction"));
    }
    // (4) Every other node's decoded bytes must be unchanged, node by node.
    let mut old_decoder = NodeDecoder::new(&bytes, &bundle);
    let mut new_decoder = NodeDecoder::new(&candidate, &check_bundle);
    old_decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    old_decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    new_decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    new_decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    for (index, (old_node, new_node)) in bundle.nodes.iter().zip(&check_bundle.nodes).enumerate() {
        super::cancelled()?;
        if old_node.offset != new_node.offset || old_node.size != new_node.size
            || old_node.flags != new_node.flags || old_node.path != new_node.path {
            return Err(invalid("Unity texture replace changed the bundle node table"));
        }
        if index == node_index { continue; }
        let mut position = 0u64;
        while position < old_node.size {
            super::cancelled()?;
            let take = (4 * 1024 * 1024u64).min(old_node.size - position);
            let before = old_decoder.read(old_node.offset + position, take)?;
            let after = new_decoder.read(new_node.offset + position, take)?;
            if before != after {
                return Err(invalid("Unity texture replace changed a different bundle node"));
            }
            position += take;
        }
    }
    // Publish only a strictly smaller container: the route's purpose is space,
    // and a rebuild that is not smaller is reported as NO_GAIN with nothing
    // written (the same rule the lossless recompression export uses).
    source.unchanged()?;
    let status = if candidate.len() < bytes.len() { "EXPORTED" } else { "NO_GAIN" };
    if status == "EXPORTED" {
        super::detached_export::publish(output, &candidate, || source.unchanged())?;
    }
    println!("UNITYFS_TEXTURE_REPLACE|{node_index}|{path_id}|{}x{}|{new_width}x{new_height}|{}|{new_mips}|{payload_len}|{new_len}|{}|{}|{status}",
        old_texture.width, old_texture.height, old_texture.mips, bytes.len(), candidate.len());
    eprintln!("Experimental detached Unity texture replace: source unchanged; the supplied payload bytes are spliced verbatim (no pixel/codec re-encode) and a shrink keeps the object extent by zero-padding the tail. NO_GAIN means the rebuilt container is not strictly smaller and nothing was published. Only an all-zero uncompressed-data hash field is accepted and it is preserved, not recomputed; no CRC/CAB-name/Addressables-catalogue rebuild, no .resS relocation, no proof that the Unity runtime accepts a zero-padded object extent and no runtime validation. Research artifact only.");
    Ok(())
}

/// Hard bound on the number of streamed Texture2D objects one physical
/// compaction may have to account for across the whole bundle. Exceeding it
/// blocks compaction (the zero-padded fallback still runs) instead of looping.
#[cfg(any(test, feature = "development-audits"))]
const MAX_STREAM_REFS: usize = 4096;

/// One streamed texture reference whose declared `m_StreamData/offset` must be
/// rewritten by a physical compaction: the SerializedFile node it lives in, its
/// path ID, the NEW absolute position of its 8-byte offset field in the decoded
/// bundle, the old node-relative offset it must currently hold, and the field's
/// endianness.
#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug)]
struct StreamShiftRef {
    sf_node: usize,
    id: u64,
    field_at: usize,
    old_offset: u64,
    big: bool,
}

#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug)]
struct StreamCompactionPlan {
    shifts: Vec<StreamShiftRef>,
    streamed: usize,
}

/// Prove that every streamed Texture2D in this bundle that can reference the
/// target `.resS` node can be rewritten by one physical removal of
/// `[stream_offset + new_len, stream_offset + stream_size)` and list exactly
/// what blocks it otherwise. The rules are fail-closed:
///   * every SerializedFile node must be readable and fully inventoried: an
///     opaque, unsupported, over-budget or unretained texture object could hide
///     a stream range, so any of them blocks compaction;
///   * every streamed object's path must resolve to a node in this bundle
///     (another resource node is fine: it is not being compacted);
///   * an object in the target node whose range lies inside the removed region
///     or straddles its start blocks compaction (its bytes would disappear);
///   * an object in the target node whose range starts at or after the removed
///     end must have a captured 8-byte `m_StreamData/offset` span that currently
///     holds exactly the parsed offset, so the field can be shifted by the
///     removed length;
///   * nodes with flags other than 0 (resource) or 4 (SerializedFile) cannot be
///     proven to hold no SerializedFile objects and block compaction;
///   * more than `MAX_STREAM_REFS` streamed objects blocks compaction.
/// Nothing is written; the caller keeps the zero-padded route as fallback.
#[cfg(any(test, feature = "development-audits"))]
#[allow(clippy::too_many_arguments)]
fn plan_stream_compaction(bytes: &[u8], bundle: &Bundle, node_index: usize, path_id: u64,
    target: usize, stream_offset: u64, stream_size: u64, new_len: usize)
    -> Result<StreamCompactionPlan, Vec<String>> {
    let mut blockers = Vec::new();
    let mut shifts = Vec::new();
    let removed = stream_size - new_len as u64;
    let new_end = stream_offset + new_len as u64;
    let old_end = stream_offset + stream_size;
    let target_node = &bundle.nodes[target];
    let rem_to = target_node.offset + old_end;
    let mut names = std::collections::BTreeMap::<Vec<u8>, usize>::new();
    for (index, node) in bundle.nodes.iter().enumerate() {
        if names.insert(node.path.clone(), index).is_some() {
            return Err(vec![format!("duplicate node name {} makes stream resolution ambiguous",
                String::from_utf8_lossy(&node.path))]);
        }
    }
    let mut decoder = NodeDecoder::new(bytes, bundle);
    decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    let mut streamed = 0usize;
    'nodes: for (index, node) in bundle.nodes.iter().enumerate() {
        super::cancelled().map_err(|error| vec![format!("cancelled: {error}")])?;
        match node.flags {
            // Resource nodes hold no SerializedFile objects, so they cannot
            // carry a Texture2D stream range; other flags are unproven.
            0 => (),
            4 => {
                let name = String::from_utf8_lossy(&node.path).into_owned();
                if node.size > 64 * 1024 * 1024 {
                    blockers.push(format!("node {index} ({name}): SerializedFile exceeds the 64 MiB read limit"));
                    continue;
                }
                let slice = match decoder.read(node.offset, node.size) {
                    Ok(slice) => slice,
                    Err(error) => { blockers.push(format!("node {index} ({name}): cannot be read: {error}")); continue; }
                };
                if !crate::unity_serialized::plausible(&slice) {
                    blockers.push(format!("node {index} ({name}): not a supported SerializedFile header"));
                    continue;
                }
                let inventory = match node_texture_inventory(&slice) {
                    Ok(inventory) => inventory,
                    Err(error) => { blockers.push(format!("node {index} ({name}): texture inventory failed: {error}")); continue; }
                };
                if inventory.unretained != 0 {
                    blockers.push(format!("node {index} ({name}): {} texture object(s) were not retained by the inventory; their stream ranges cannot be proven", inventory.unretained));
                }
                for (id, status) in &inventory.opaque {
                    blockers.push(format!("node {index} ({name}): texture object {id} is not retained ({status}); its stream range cannot be proven"));
                }
                for (id, fields) in &inventory.details {
                    let texture = &fields.texture;
                    if texture.stream_size == 0 { continue; }
                    streamed += 1;
                    if streamed > MAX_STREAM_REFS {
                        blockers.push(format!("the bundle declares more than the {MAX_STREAM_REFS} streamed textures compaction is bounded to"));
                        break 'nodes;
                    }
                    let Some(stream_name) = stream_node(&node.path, &texture.stream_path) else {
                        blockers.push(format!("node {index} object {id}: stream path {:?} does not resolve in this bundle", texture.stream_path));
                        continue;
                    };
                    let Some(&resolved) = names.get(stream_name) else {
                        blockers.push(format!("node {index} object {id}: stream target {} is not present in this bundle", String::from_utf8_lossy(stream_name)));
                        continue;
                    };
                    if bundle.nodes[resolved].flags != 0 {
                        blockers.push(format!("node {index} object {id}: stream target is not a resource node"));
                        continue;
                    }
                    let Some(end) = texture.stream_offset.checked_add(texture.stream_size) else {
                        blockers.push(format!("node {index} object {id}: stream extent overflows"));
                        continue;
                    };
                    if end > bundle.nodes[resolved].size {
                        blockers.push(format!("node {index} object {id}: stream extent [{}, {}) is outside its target node", texture.stream_offset, end));
                        continue;
                    }
                    if resolved != target { continue; }
                    if *id == path_id {
                        if index != node_index || texture.stream_offset != stream_offset || texture.stream_size != stream_size {
                            blockers.push(format!("node {index} object {id}: target identity differs from the located object"));
                        }
                        continue;
                    }
                    if end <= new_end { continue; }
                    if texture.stream_offset >= old_end {
                        let Some(span) = fields.spans.get("m_StreamData/offset") else {
                            blockers.push(format!("node {index} object {id}: the m_StreamData/offset span is missing"));
                            continue;
                        };
                        if span.kind != "PRIMITIVE_BITS" || span.end != span.start + 8 {
                            blockers.push(format!("node {index} object {id}: the m_StreamData/offset span cannot be proven to be an 8-byte primitive"));
                            continue;
                        }
                        let Some(record) = inventory.object_records.iter().find(|record| record.id == *id) else {
                            blockers.push(format!("node {index} object {id}: object record was not retained"));
                            continue;
                        };
                        let at = record.start as usize + span.start;
                        if at.checked_add(8).is_none_or(|field_end| field_end > slice.len())
                            || read_u64(&slice, at, inventory.big).ok() != Some(texture.stream_offset) {
                            blockers.push(format!("node {index} object {id}: the offset span does not hold the parsed stream offset"));
                            continue;
                        }
                        let Some(new_node_offset) = (if node.offset >= rem_to { node.offset.checked_sub(removed) } else { Some(node.offset) }) else {
                            blockers.push(format!("node {index} object {id}: node offset cannot be shifted"));
                            continue;
                        };
                        let Some(field_at) = (new_node_offset as usize).checked_add(at) else {
                            blockers.push(format!("node {index} object {id}: shifted field offset overflows"));
                            continue;
                        };
                        shifts.push(StreamShiftRef { sf_node: index, id: *id, field_at,
                            old_offset: texture.stream_offset, big: inventory.big });
                        continue;
                    }
                    let reason = if texture.stream_offset >= new_end {
                        "lies inside the removed region"
                    } else {
                        "straddles the start of the removed region"
                    };
                    blockers.push(format!("node {index} object {id}: declared range [{}, {}) {reason}", texture.stream_offset, end));
                }
            }
            other => blockers.push(format!("node {index}: flags {other} (not a resource or SerializedFile) cannot be proven to hold no SerializedFile objects")),
        }
    }
    if !blockers.is_empty() { return Err(blockers); }
    Ok(StreamCompactionPlan { shifts, streamed })
}

/// Result of re-proving a compacted candidate before publication.
#[cfg(any(test, feature = "development-audits"))]
struct CompactionVerification {
    gain: u64,
    refs_shifted: usize,
    streamed: usize,
}

/// Re-prove a compacted candidate against the source bundle: the `.resS` node
/// lost exactly the removed range (new payload at the same offset, every other
/// byte unchanged), every non-`.resS` node is byte-identical outside the exactly
/// declared patches, the SerializedFile object records and every retained
/// texture identity are byte-stable, every streamed texture still extracts
/// byte-identically except the target (which must equal the supplied payload),
/// every planned offset shift was observed, and the declared streamed total
/// shrank by exactly the removed length. Returns the honest gain and counts.
#[cfg(any(test, feature = "development-audits"))]
#[allow(clippy::too_many_arguments)]
fn verify_compacted_stream(bytes: &[u8], bundle: &Bundle, candidate: &[u8], check_bundle: &Bundle,
    node_index: usize, path_id: u64, expected_node: &[u8], patch_lo: usize, patch_hi: usize,
    target: usize, stream_offset: u64, stream_size: u64, new_payload: &[u8],
    new_width: u32, new_height: u32, new_mips: u32, plan: &StreamCompactionPlan)
    -> io::Result<CompactionVerification> {
    let removed = stream_size - new_payload.len() as u64;
    let target_node = &bundle.nodes[target];
    let check_target = &check_bundle.nodes[target];
    if check_target.offset != target_node.offset || check_target.size + removed != target_node.size
        || check_target.flags != target_node.flags || check_target.path != target_node.path {
        return Err(invalid("Unity streamed compaction changed the .resS node extent"));
    }
    for (index, (old, new)) in bundle.nodes.iter().zip(&check_bundle.nodes).enumerate() {
        if old.flags != new.flags || old.path != new.path {
            return Err(invalid("Unity streamed compaction changed a bundle node entry"));
        }
        let expected = if index == target {
            (old.offset, old.size - removed)
        } else if old.offset >= target_node.offset + stream_offset + stream_size {
            (old.offset.checked_sub(removed).ok_or_else(|| invalid("Unity streamed compaction node underflow"))?, old.size)
        } else {
            (old.offset, old.size)
        };
        if (new.offset, new.size) != expected {
            return Err(invalid("Unity streamed compaction did not shift the node table exactly"));
        }
    }
    let mut old_decoder = NodeDecoder::new(bytes, bundle);
    let mut new_decoder = NodeDecoder::new(candidate, check_bundle);
    for decoder in [&mut old_decoder, &mut new_decoder] {
        decoder.decode_budget = 3 * (MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024);
        decoder.copy_budget = 3 * (MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024);
    }
    // (1) The .resS node must hold exactly the new payload at the same offset
    // followed by the source bytes that used to follow the old extent.
    let kept_end = stream_offset + new_payload.len() as u64;
    let old_extent_end = stream_offset + stream_size;
    let mut position = 0u64;
    while position < check_target.size {
        super::cancelled()?;
        let take = (4 * 1024 * 1024u64).min(check_target.size - position);
        let mut expected = Vec::with_capacity(take as usize);
        if position < kept_end {
            let length = kept_end.min(position + take) - position;
            expected.extend_from_slice(&old_decoder.read(target_node.offset + position, length)?);
        }
        if position + take > kept_end {
            let start = kept_end.max(position);
            let length = position + take - start;
            // New coordinate `start` maps to old coordinate `start + removed`.
            expected.extend_from_slice(&old_decoder.read(target_node.offset + start + removed, length)?);
        }
        let first = position.max(stream_offset);
        let last = (position + take).min(kept_end);
        if first < last {
            expected[(first - position) as usize..(last - position) as usize]
                .copy_from_slice(&new_payload[(first - stream_offset) as usize..(last - stream_offset) as usize]);
        }
        let actual = new_decoder.read(check_target.offset + position, take)?;
        if actual != expected {
            return Err(invalid("Unity streamed compaction changed .resS bytes outside the removed extent"));
        }
        position += take;
    }
    if check_target.size != target_node.size - removed || target_node.size < old_extent_end {
        return Err(invalid("Unity streamed compaction .resS node size is inconsistent"));
    }
    // (2) Patches in NEW absolute coordinates: the target's own fields and every
    // shifted offset field. They must all lie in SerializedFile nodes.
    let mut patches: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let sf_new_offset = check_bundle.nodes[node_index].offset as usize;
    patches.push((sf_new_offset + patch_lo, sf_new_offset + patch_hi, expected_node[patch_lo..patch_hi].to_vec()));
    for shift in &plan.shifts {
        let value = shift.old_offset - removed;
        let bytes = if shift.big { value.to_be_bytes() } else { value.to_le_bytes() };
        patches.push((shift.field_at, shift.field_at + 8, bytes.to_vec()));
    }
    for (start, end, _) in &patches {
        if *start >= *end || *end > check_bundle.decoded as usize { return Err(invalid("Unity streamed compaction patch extent is invalid")); }
    }
    // (3) Every non-.resS node must be byte-identical outside those patches; the
    // target's SerializedFile node must equal the spliced expectation exactly.
    let mut patched_nodes = std::collections::BTreeSet::new();
    for (start, end, _) in &patches {
        let mut found = false;
        for (index, node) in check_bundle.nodes.iter().enumerate() {
            if index == target { continue; }
            let node_start = node.offset as usize;
            let node_end = node_start + node.size as usize;
            if *start >= node_start && *end <= node_end { found = true; patched_nodes.insert(index); break; }
        }
        if !found { return Err(invalid("Unity streamed compaction patch lies outside every SerializedFile node")); }
    }
    for (index, old_node) in bundle.nodes.iter().enumerate() {
        super::cancelled()?;
        if index == target { continue; }
        let new_node = &check_bundle.nodes[index];
        let node_patches: Vec<&(usize, usize, Vec<u8>)> = patches.iter()
            .filter(|(start, end, _)| *start >= new_node.offset as usize && *end <= (new_node.offset + new_node.size) as usize)
            .collect();
        if index == node_index {
            // The target's SerializedFile node carries its own field patch and
            // any shifted stream offsets that live in it; both must be spliced
            // into the expectation before the byte comparison.
            let base = new_node.offset as usize;
            let mut expected = expected_node.to_vec();
            for (start, end, bytes) in &node_patches {
                expected[*start - base..*end - base].copy_from_slice(bytes);
            }
            let actual = new_decoder.read(new_node.offset, new_node.size)?;
            if actual != expected {
                return Err(invalid("Unity streamed compaction changed the target SerializedFile node"));
            }
            continue;
        }
        let mut position = 0u64;
        while position < old_node.size {
            super::cancelled()?;
            let take = (4 * 1024 * 1024u64).min(old_node.size - position);
            let mut expected = old_decoder.read(old_node.offset + position, take)?;
            for (start, end, bytes) in &node_patches {
                let first = (*start).max((new_node.offset + position) as usize);
                let last = (*end).min((new_node.offset + position + take) as usize);
                if first < last {
                    let base = new_node.offset as usize + position as usize;
                    expected[first - base..last - base].copy_from_slice(&bytes[first - *start..last - *start]);
                }
            }
            let actual = new_decoder.read(new_node.offset + position, take)?;
            if actual != expected {
                return Err(invalid("Unity streamed compaction changed a different bundle node"));
            }
            position += take;
        }
    }
    // (4) The object tables and every retained texture identity must re-run
    // byte-stable, with only the target's declared shape/size and the planned
    // offset fields changed.
    let shifts: std::collections::BTreeMap<(usize, u64), u64> = plan.shifts.iter()
        .map(|shift| ((shift.sf_node, shift.id), shift.old_offset - removed)).collect();
    let mut filtered = Vec::new(); // (node index, source slice, source inventory)
    for (index, old_node) in bundle.nodes.iter().enumerate() {
        if old_node.flags != 4 { continue; }
        let old_slice = old_decoder.read(old_node.offset, old_node.size)?;
        let new_slice = new_decoder.read(check_bundle.nodes[index].offset, check_bundle.nodes[index].size)?;
        let before = node_texture_inventory(&old_slice)?;
        let after = node_texture_inventory(&new_slice)?;
        if before.version != after.version || before.big != after.big || before.trees != after.trees
            || before.textures != after.textures || before.unretained != after.unretained
            || before.opaque.len() != after.opaque.len() || before.details.len() != after.details.len()
            || before.object_records.len() != after.object_records.len() {
            return Err(invalid("Unity streamed compaction changed the SerializedFile object table"));
        }
        for (old, new) in before.object_records.iter().zip(&after.object_records) {
            if (old.id, old.start, old.size, old.type_index, old.table_start, old.table_end)
                != (new.id, new.start, new.size, new.type_index, new.table_start, new.table_end) {
                return Err(invalid("Unity streamed compaction changed a SerializedFile object entry"));
            }
        }
        for ((id, before_fields), (after_id, after_fields)) in before.details.iter().zip(&after.details) {
            if id != after_id || before_fields.image_count != after_fields.image_count
                || before_fields.dimension != after_fields.dimension {
                return Err(invalid("Unity streamed compaction changed a retained texture identity"));
            }
            let (b, a) = (&before_fields.texture, &after_fields.texture);
            if a.format != b.format || a.stream_path != b.stream_path || a.inline_bytes != b.inline_bytes {
                return Err(invalid("Unity streamed compaction changed a retained texture's storage kind"));
            }
            if *id == path_id {
                if (a.width, a.height, a.mips, a.stream_size, a.stream_offset)
                    != (new_width, new_height, new_mips, new_payload.len() as u64, stream_offset) {
                    return Err(invalid("Unity streamed compaction did not re-parse the target at its new declared shape"));
                }
            } else {
                if (a.width, a.height, a.mips, a.stream_size) != (b.width, b.height, b.mips, b.stream_size) {
                    return Err(invalid("Unity streamed compaction changed an unrelated texture's declared shape"));
                }
                let expected = shifts.get(&(index, *id)).copied().unwrap_or(b.stream_offset);
                if a.stream_offset != expected {
                    return Err(invalid("Unity streamed compaction shifted an offset that was not planned or missed one"));
                }
            }
        }
        filtered.push((index, old_slice, before));
    }
    // (5) Extract every streamed texture from the source and the candidate: all
    // must be byte-identical except the target, which must equal the payload.
    let mut names = std::collections::BTreeMap::<Vec<u8>, usize>::new();
    for (index, node) in bundle.nodes.iter().enumerate() { names.insert(node.path.clone(), index); }
    let mut observed = std::collections::BTreeSet::new();
    let mut streamed = 0usize;
    let (mut source_total, mut candidate_total) = (0u64, 0u64);
    for (index, _, before) in &filtered {
        let node = &bundle.nodes[*index];
        for (id, fields) in &before.details {
            let texture = &fields.texture;
            if texture.stream_size == 0 { continue; }
            streamed += 1;
            let name = stream_node(&node.path, &texture.stream_path)
                .ok_or_else(|| invalid("Unity streamed compaction lost a stream path resolution"))?;
            let resolved = *names.get(name).ok_or_else(|| invalid("Unity streamed compaction lost a stream target"))?;
            let (new_offset, new_size) = if *id == path_id {
                (stream_offset, new_payload.len() as u64)
            } else if let Some(&shifted) = shifts.get(&(*index, *id)) {
                observed.insert((*index, *id));
                (shifted, texture.stream_size)
            } else {
                (texture.stream_offset, texture.stream_size)
            };
            let source_bytes = old_decoder.read(bundle.nodes[resolved].offset + texture.stream_offset, texture.stream_size)?;
            let candidate_bytes = new_decoder.read(check_bundle.nodes[resolved].offset + new_offset, new_size)?;
            source_total += texture.stream_size;
            candidate_total += new_size;
            if *id == path_id {
                if candidate_bytes != new_payload {
                    return Err(invalid("Unity streamed compaction payload failed re-extraction"));
                }
            } else if candidate_bytes != source_bytes {
                return Err(invalid("Unity streamed compaction changed an unrelated streamed payload"));
            }
        }
    }
    if observed.len() != shifts.len() {
        return Err(invalid("Unity streamed compaction did not observe every planned offset shift"));
    }
    if streamed != plan.streamed || source_total - candidate_total != removed {
        return Err(invalid("Unity streamed compaction streamed-size accounting is inconsistent"));
    }
    Ok(CompactionVerification { gain: bytes.len().saturating_sub(candidate.len()) as u64,
        refs_shifted: shifts.len(), streamed })
}

/// Result of an in-memory streamed shrink: the verified candidate bundle, how
/// the freed tail was handled and the honest accounting the callers report.
#[cfg(any(test, feature = "development-audits"))]
#[derive(Clone, Copy, PartialEq, Debug)]
enum StreamedShrinkMode { Compacted, ZeroPadded }

#[cfg(any(test, feature = "development-audits"))]
struct StreamedShrinkOutcome {
    candidate: Vec<u8>,
    mode: StreamedShrinkMode,
    removed: u64,
    refs_shifted: usize,
    streamed: usize,
    touched_blocks: usize,
    touched_old: u64,
    touched_new: u64,
    diagnostics: Vec<String>,
}

/// Rebuild one bundle in memory with a single streamed Texture2D shrunk to
/// `new_payload`: the preferred physical compaction when every reference can be
/// proven, otherwise the verified zero-padded in-place rewrite. This is the
/// verified core of both the detached `unityfs-texture-replace` route and the
/// in-place bundle apply; it writes nothing and publishes nothing. Any failure
/// returns an error and no candidate. `diagnostics` explains every compaction
/// blocker for the caller to report.
#[cfg(any(test, feature = "development-audits"))]
fn streamed_shrink_candidate(bytes: &[u8], bundle: &Bundle, located: &LocatedTexture,
    node_index: usize, path_id: u64, new_payload: &[u8], new_width: u32, new_height: u32, new_mips: u32)
    -> io::Result<StreamedShrinkOutcome> {
    let old_texture = &located.fields.texture;
    let (target, stream_offset, stream_size) = match located.storage {
        LocatedStorage::Streamed { target, offset, size } => (target, offset, size),
        LocatedStorage::Inline { .. } => return Err(invalid("Unity streamed shrink requires streamed storage")),
    };
    let new_len = new_payload.len();
    if new_len as u64 >= stream_size {
        return Err(invalid("Unity streamed shrink payload must be strictly shorter than the current stream extent"));
    }
    let mut diagnostics: Vec<String> = Vec::new();
    let mut expected_node = located.slice.clone();
    let patches_for_object = streamed_shrink_patches(located, new_len, new_width, new_height, new_mips)?;
    for patch in &patches_for_object {
        expected_node[patch.start..patch.end].copy_from_slice(&patch.bytes);
    }
    let patch_lo = patches_for_object.iter().map(|patch| patch.start).min()
        .ok_or_else(|| invalid("Unity streamed shrink produced no patches"))?;
    let patch_hi = patches_for_object.iter().map(|patch| patch.end).max()
        .ok_or_else(|| invalid("Unity streamed shrink produced no patches"))?;
    let target_node = &bundle.nodes[target];
    let stream_patch_start = (target_node.offset + stream_offset) as usize;
    if stream_patch_start.checked_add(stream_size as usize).is_none_or(|end| end > bundle.decoded as usize) {
        return Err(invalid("Unity streamed extent is outside the decoded bundle"));
    }
    // Preferred route: physically remove `[stream_patch_start + new_len,
    // stream_patch_start + stream_size)` and shift every proven later offset.
    match plan_stream_compaction(bytes, bundle, node_index, path_id, target, stream_offset, stream_size, new_len) {
        Ok(plan) => {
            let removed = stream_size - new_len as u64;
            let rem_from = stream_patch_start + new_len;
            let rem_to = stream_patch_start + stream_size as usize;
            let sf_new_offset = if bundle.nodes[node_index].offset >= rem_to as u64 {
                bundle.nodes[node_index].offset - removed
            } else { bundle.nodes[node_index].offset };
            let mut patches: Vec<(usize, Vec<u8>)> = Vec::new();
            patches.push((sf_new_offset as usize + patch_lo, expected_node[patch_lo..patch_hi].to_vec()));
            for shift in &plan.shifts {
                let value = shift.old_offset - removed;
                let bytes = if shift.big { value.to_be_bytes() } else { value.to_le_bytes() };
                patches.push((shift.field_at, bytes.to_vec()));
            }
            patches.push((stream_patch_start, new_payload.to_vec()));
            patches.sort_unstable_by_key(|(start, _)| *start);
            let patch_refs: Vec<(usize, &[u8])> = patches.iter()
                .map(|(start, bytes)| (*start, bytes.as_slice())).collect();
            let attempt = rebuild_compacted(bytes, &patch_refs, rem_from, rem_to, target)
                .and_then(|(candidate, stats, _source_bundle, check_bundle)| {
                    let verification = verify_compacted_stream(bytes, bundle, &candidate, &check_bundle,
                        node_index, path_id, &expected_node, patch_lo, patch_hi, target,
                        stream_offset, stream_size, &new_payload, new_width, new_height, new_mips, &plan)?;
                    // The decoded bundle shrank by exactly the removed delta (the
                    // compaction's whole point); the FILE gain is the encoded
                    // footprint of the removed bytes minus the touched blocks'
                    // re-encode overhead, so it is checked for strictness and
                    // reported honestly rather than assumed equal to the delta.
                    if check_bundle.decoded + removed != bundle.decoded {
                        return Err(invalid("Unity streamed compaction did not remove exactly the declared delta"));
                    }
                    let touched_old: u64 = stats.touched.iter().map(|(_, old, _)| *old as u64).sum();
                    let touched_new: u64 = stats.touched.iter().map(|(_, _, new)| *new as u64).sum();
                    Ok((candidate, stats, verification, touched_old, touched_new))
                });
            match attempt {
                Ok((candidate, stats, verification, touched_old, touched_new)) => {
                    return Ok(StreamedShrinkOutcome {
                        candidate, mode: StreamedShrinkMode::Compacted,
                        removed: stats.removed as u64, refs_shifted: verification.refs_shifted,
                        streamed: verification.streamed, touched_blocks: stats.touched.len(),
                        touched_old, touched_new, diagnostics });
                }
                Err(error) => diagnostics.push(format!("candidate rejected: {error}")),
            }
        }
        Err(blockers) => {
            if blockers.is_empty() {
                diagnostics.push("compaction was blocked without a stated reason".to_string());
            } else {
                diagnostics.extend(blockers);
            }
        }
    }
    // Fallback: the zero-padded in-place rewrite, unchanged from the prior route.
    let node_patch = (located.node_offset as usize + patch_lo, &expected_node[patch_lo..patch_hi]);
    let mut stream_patch = new_payload.to_vec();
    stream_patch.resize(stream_size as usize, 0);
    // Two disjoint patches in one rebuild: the SerializedFile node's fields and
    // the .resS extent. Each node keeps its own codec and the block/node tables
    // and info section are rebuilt exactly once.
    let (candidate, _changed, _) = rebuild_patches(bytes,
        &[node_patch, (stream_patch_start, &stream_patch)])?;
    // (1) The candidate must re-parse as a UnityFS bundle with the same layout.
    let check_bundle = parse(&candidate)?;
    if check_bundle.flags != bundle.flags || check_bundle.nodes.len() != bundle.nodes.len()
        || check_bundle.blocks.len() != bundle.blocks.len() || check_bundle.decoded != bundle.decoded {
        return Err(invalid("Unity texture replace changed the bundle layout"));
    }
    // (2) Every retained object entry must be byte-stable and the target must
    // re-parse with the new declared shape.
    let mut check_decoder = NodeDecoder::new(&candidate, &check_bundle);
    check_decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    check_decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    let check_node = &check_bundle.nodes[node_index];
    if check_node.offset != bundle.nodes[node_index].offset || check_node.size != bundle.nodes[node_index].size
        || check_node.flags != bundle.nodes[node_index].flags || check_node.path != bundle.nodes[node_index].path {
        return Err(invalid("Unity texture replace changed the bundle node table"));
    }
    let check_slice = check_decoder.read(check_node.offset, check_node.size)?;
    let before_inventory = node_texture_inventory(&located.slice)?;
    let after_inventory = node_texture_inventory(&check_slice)?;
    if after_inventory.version != before_inventory.version || after_inventory.big != before_inventory.big
        || after_inventory.textures != before_inventory.textures
        || after_inventory.object_records.len() != before_inventory.object_records.len() {
        return Err(invalid("Unity texture replace changed the SerializedFile object table"));
    }
    for (before, after) in before_inventory.object_records.iter().zip(&after_inventory.object_records) {
        if before.id != after.id || before.start != after.start || before.size != after.size
            || before.type_index != after.type_index || before.table_start != after.table_start
            || before.table_end != after.table_end {
            return Err(invalid("Unity texture replace changed a SerializedFile object entry"));
        }
    }
    let (_, declared) = after_inventory.details.iter().find(|(id, _)| *id == path_id)
        .ok_or_else(|| invalid("Unity texture replace target no longer re-parses as a supported texture"))?;
    if declared.texture.width != new_width || declared.texture.height != new_height
        || declared.texture.mips != new_mips || declared.texture.format != old_texture.format
        || declared.texture.inline_bytes != 0
        || declared.texture.stream_offset != stream_offset
        || declared.texture.stream_size != new_len as u64
        || declared.image_count != Some(1) || declared.dimension != Some(2) {
        return Err(invalid("Unity texture replace target did not re-parse with the new declared shape"));
    }
    // (3) The new payload must re-extract byte-identically through the same
    // streamed selection the export route uses, at the same offset, and the whole
    // replaced SerializedFile node must equal the spliced expectation.
    let check = locate_texture(node_index, path_id, &check_bundle, &mut check_decoder)?;
    if check.version != located.version || check.big != located.big
        || check.expected != new_len as u64
        || check.fields.texture.width != new_width
        || check.fields.texture.height != new_height
        || check.fields.texture.format != old_texture.format
        || check.fields.texture.mips != new_mips
        || check.fields.texture.inline_bytes != 0 {
        return Err(invalid("Unity texture replace changed the located texture identity"));
    }
    let LocatedStorage::Streamed { target: checked_target, offset: checked_offset, size: checked_size } = check.storage else {
        return Err(invalid("Unity texture replace did not produce streamed storage"));
    };
    if checked_target != target || checked_offset != stream_offset || checked_size != new_len as u64
        || check.slice != expected_node {
        return Err(invalid("Unity streamed replace relocated the stream extent"));
    }
    let reextracted = check_decoder.read(check_bundle.nodes[checked_target].offset + checked_offset, checked_size)?;
    if reextracted != new_payload {
        return Err(invalid("Unity texture replace payload failed re-extraction"));
    }
    // (4) Every other node's decoded bytes must be unchanged, node by node.
    let mut old_decoder = NodeDecoder::new(bytes, bundle);
    let mut new_decoder = NodeDecoder::new(&candidate, &check_bundle);
    old_decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    old_decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    new_decoder.decode_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    new_decoder.copy_budget = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;
    for (index, (old_node, new_node)) in bundle.nodes.iter().zip(&check_bundle.nodes).enumerate() {
        super::cancelled()?;
        if old_node.offset != new_node.offset || old_node.size != new_node.size
            || old_node.flags != new_node.flags || old_node.path != new_node.path {
            return Err(invalid("Unity texture replace changed the bundle node table"));
        }
        if index == node_index || index == target { continue; }
        let mut position = 0u64;
        while position < old_node.size {
            super::cancelled()?;
            let take = (4 * 1024 * 1024u64).min(old_node.size - position);
            let before = old_decoder.read(old_node.offset + position, take)?;
            let after = new_decoder.read(new_node.offset + position, take)?;
            if before != after {
                return Err(invalid("Unity texture replace changed a different bundle node"));
            }
            position += take;
        }
    }
    // (5) The .resS node must be byte-identical outside the OLD declared extent
    // and inside it must hold the new payload followed by zeros.
    let check_target = &check_bundle.nodes[target];
    let mut position = 0u64;
    while position < target_node.size {
        super::cancelled()?;
        let take = (4 * 1024 * 1024u64).min(target_node.size - position);
        let before = old_decoder.read(target_node.offset + position, take)?;
        let after = new_decoder.read(check_target.offset + position, take)?;
        let mut expected = before;
        let first = position.max(stream_offset);
        let last = (position + take).min(stream_offset + stream_size);
        if first < last {
            expected[(first - position) as usize..(last - position) as usize]
                .copy_from_slice(&stream_patch[(first - stream_offset) as usize..(last - stream_offset) as usize]);
        }
        if after != expected {
            return Err(invalid("Unity streamed replace changed bytes outside the zeroed stream extent"));
        }
        position += take;
    }
    Ok(StreamedShrinkOutcome {
        candidate, mode: StreamedShrinkMode::ZeroPadded, removed: 0, refs_shifted: 0,
        streamed: 0, touched_blocks: 0, touched_old: 0, touched_new: 0, diagnostics })
}

/// Development-only detached rewrite of one streamed `.resS` Unity
/// SerializedFile Texture2D payload, published as a NEW bundle. Selection,
/// identity and stream resolution are exactly the export route's: same-bundle
/// node name via `stream_node`, resource node flags 0, extent in bounds and
/// uniquely claimed by this object (no other retained texture declares an
/// overlapping range into the same node). The payload must be strictly shorter
/// than the declared extent and a complete 1x1-terminated mip chain for
/// dimensions that are the current ones halved one or more times: the exact
/// shrink rule of the inline route, including the repository short-edge policy,
/// so the payload length determines the new shape uniquely and nothing is
/// guessed. The same-shape and larger cases are refused; this route is
/// shrink-only because it exists to free stream bytes.
///
/// Two publication modes:
///   * PHYSICAL COMPACTION (preferred): the freed tail `[new_end, old_end)` is
///     removed from the `.resS` node's decoded bytes, so the node becomes
///     smaller by exactly that delta, the decoded bundle shrinks by it, the node
///     table is rebuilt (target size and every later node offset) and every
///     streamed object whose declared range starts at or after `old_end` in the
///     same node has its `m_StreamData/offset` shifted down by the delta through
///     its captured span. The target's own fields are patched as usual. This is
///     only allowed when the plan in `plan_stream_compaction` proves every
///     reference; otherwise the candidate is discarded (nothing is published by
///     the compacted route) and the blockers are reported.
///   * ZERO-PADDED FALLBACK (unchanged prior behavior): the new payload is
///     written at the same stream offset and the rest of the old extent is
///     zero-filled, leaving every offset valid. Used whenever compaction cannot
///     be proven and, like compaction, only published when strictly smaller.
///
/// Verification before publication (compaction): the plan rules above, the
/// candidate re-parsed with the target node smaller by exactly the removed
/// delta and every later node shifted by exactly it, every non-`.resS` node
/// byte-identical outside the declared patches, every object record
/// `(id, start, size, type_index)` and every retained texture identity
/// byte-stable, every streamed texture re-extracted from the source and the
/// candidate and byte-identical except the target (which must equal the supplied
/// payload), every planned shift observed, the declared streamed total shrunk by
/// exactly the removed delta and the container strictly smaller. The decoded
/// gain is exactly the removed delta; because the covering blocks are LZ4, the
/// FILE gain is the encoded footprint of the removed bytes minus the touched
/// blocks' re-encode overhead, which is reported instead of overclaimed.
/// The zero-padded fallback keeps its prior verification. Any failure publishes
/// nothing. Stream ownership stays unproven, only an all-zero uncompressed-data
/// hash placeholder is accepted (preserved, not recomputed), no CRC/CAB-name/
/// Addressables catalogue is rebuilt and the artifact is not runtime validated.
#[cfg(any(test, feature = "development-audits"))]
fn replace_streamed_texture(bytes: &[u8], bundle: &Bundle, located: &LocatedTexture,
    node_index: usize, path_id: u64, payload_path: &Path, output: &Path,
    source: &crate::audit_io::Source) -> io::Result<()> {
    use crate::audit_io::Source;
    let old_texture = &located.fields.texture;
    let stream_size = match located.storage {
        LocatedStorage::Streamed { size, .. } => size,
        LocatedStorage::Inline { .. } => return Err(invalid("Unity streamed replace requires streamed storage")),
    };
    let object_start = located.record.start as usize;
    // Streamed textures carry zero inline bytes; the image-data length prefix
    // must be a proven zero-length field so no inline payload is silently
    // ignored, and the locate route already refused a texture that declares both.
    let span = located.fields.spans.get("image data")
        .or_else(|| located.fields.spans.get("image data/Array"))
        .ok_or_else(|| invalid("Unity streamed texture is missing its image data span"))?;
    if span.payload != Some((span.start + 4, 0))
        || read_u32(&located.slice, object_start + span.start, located.big)? != 0 {
        return Err(invalid("Unity streamed texture has nonzero inline image bytes"));
    }
    // Read the supplied payload once; these bytes are spliced verbatim.
    let mut payload_source = Source::open(payload_path, MAX_TEXTURE_BUNDLE, MAX_TEXTURE_BUNDLE + 16)?;
    let new_len = payload_source.len();
    if new_len > u32::MAX as u64 {
        return Err(invalid("Unity replacement payload exceeds the 4 GiB length prefix"));
    }
    let new_len = new_len as usize;
    let new_payload = payload_source.read_at(0, new_len)?;
    // A streamed replacement is shrink-only: the new payload replaces the head of
    // the OLD declared extent and the tail is freed, so a payload that is not
    // strictly shorter cannot free anything.
    if new_len as u64 >= stream_size {
        return Err(invalid(&format!(
            "Unity streamed replacement payload must be strictly shorter than the current {stream_size}-byte stream extent; {new_len} bytes given")));
    }
    let Some((new_width, new_height, new_mips)) = shrink_shape(old_texture, new_len)? else {
        return Err(invalid(&format!(
            "Unity streamed replacement payload must be the complete 1x1-terminated mip chain of a downscale of the declared {}x{} format {} {} mip shape (both dimensions halved one or more times, short edge at least {}); {new_len} bytes given",
            old_texture.width, old_texture.height, old_texture.format, old_texture.mips,
            crate::texture_policy::MIN_SHORT_EDGE)));
    };
    prove_shrink_chain(located.format, new_width, new_height, new_mips, &new_payload)?;
    let outcome = streamed_shrink_candidate(bytes, bundle, &located, node_index, path_id,
        &new_payload, new_width, new_height, new_mips)?;
    for diagnostic in &outcome.diagnostics {
        println!("UNITYFS_STREAM_COMPACTION_BLOCKED|{node_index}|{path_id}|{diagnostic}");
    }
    if outcome.mode == StreamedShrinkMode::ZeroPadded {
        println!("UNITYFS_STREAM_COMPACTION|{node_index}|{path_id}|BLOCKED|ZERO_PADDED_FALLBACK");
    }
    // Publish only a strictly smaller container: the route's purpose is space,
    // and a rebuild that is not smaller is reported as NO_GAIN with nothing
    // written (the same rule the lossless recompression export uses).
    source.unchanged()?;
    let status = if outcome.candidate.len() < bytes.len() { "EXPORTED" } else { "NO_GAIN" };
    if status == "EXPORTED" {
        super::detached_export::publish(output, &outcome.candidate, || source.unchanged())?;
    }
    if outcome.mode == StreamedShrinkMode::Compacted {
        println!("UNITYFS_TEXTURE_REPLACE|{node_index}|{path_id}|storage=stream-compacted|{}x{}|{}x{}|{}|{}|{}|{}|{}|{}|{}|{}|{status}",
            old_texture.width, old_texture.height, new_width, new_height,
            old_texture.mips, new_mips, stream_size, new_len, outcome.removed,
            outcome.refs_shifted, bytes.len(), outcome.candidate.len());
        println!("UNITYFS_STREAM_COMPACTION|{node_index}|{path_id}|{}|{}|{}|{}|{}|{}",
            outcome.refs_shifted, outcome.streamed, outcome.removed,
            outcome.touched_blocks, outcome.touched_old, outcome.touched_new);
        eprintln!("Experimental detached Unity streamed texture replace (physical compaction): source unchanged; the freed .resS tail is removed from the node, every later node offset and every proven texture stream offset shifts down by the same delta, and the supplied payload bytes are spliced verbatim (no pixel/codec re-encode). NO_GAIN means the rebuilt container is not strictly smaller and nothing was published. The decoded bundle shrinks by exactly the removed delta; the file saving is the encoded footprint of those bytes minus the touched blocks' re-encode overhead (reported above). Only an all-zero uncompressed-data hash field is accepted and it is preserved, not recomputed; no CRC/CAB-name/Addressables-catalogue rebuild, stream ownership is unique-claim only, no proof that the Unity runtime accepts the rewritten stream layout and no runtime validation. Research artifact only.");
        return Ok(());
    }
    println!("UNITYFS_TEXTURE_REPLACE|{node_index}|{path_id}|storage=stream|{}x{}|{}x{}|{}|{}|{}|{}|{}|{}|{status}",
        old_texture.width, old_texture.height, new_width, new_height,
        old_texture.mips, new_mips, stream_size, new_len, bytes.len(), outcome.candidate.len());
    eprintln!("Experimental detached Unity streamed texture replace: source unchanged; the supplied payload bytes are spliced verbatim at the same .resS offset (no pixel/codec re-encode) and the rest of the old stream extent is zero-filled. NO_GAIN means the rebuilt container is not strictly smaller and nothing was published. Only an all-zero uncompressed-data hash field is accepted and it is preserved, not recomputed; no CRC/CAB-name/Addressables-catalogue rebuild, stream ownership and the runtime acceptance of a zero-padded stream tail are unproven, and there is no runtime validation. Research artifact only.");
    Ok(())
}

/// Resolve only exact same-bundle names and one explicitly constrained archive
/// namespace. Never strip a basename, consult the host filesystem or infer that
/// a missing companion belongs to a different bundle/player directory.
fn stream_node<'a>(serialized: &[u8], path: &'a str) -> Option<&'a [u8]> {
    let name = if let Some(tail) = path.strip_prefix("archive:/") {
        let (archive, name) = tail.split_once('/')?;
        if archive.as_bytes() != serialized { return None; }
        name
    } else {
        path
    };
    if name.is_empty() || name.starts_with('/') || name.chars().any(|c| matches!(c, '\\' | ':'))
        || name.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return None;
    }
    Some(name.as_bytes())
}

struct TextureNodeInventory {
    node: usize,
    inventory: crate::unity_serialized::BundleTextureInventory,
}
struct TextureStream {
    node: usize,
    id: u64,
    target: Option<usize>,
    offset: u64,
    size: u64,
    status: &'static str,
    shape_status: &'static str,
    expected: Option<u64>,
    actual: u64,
}

/// Cache only the most recently decoded block. Node offsets live in the decoded
/// bundle address space; their compressed-file offsets cannot be used directly.
/// Budgets charge repeated decoding/copying too, not only unique payload bytes.
struct NodeDecoder<'a> {
    source: &'a [u8], bundle: &'a Bundle, starts: Vec<u64>,
    cached: Option<(usize, Vec<u8>)>, decode_budget: u64, copy_budget: u64,
}
impl<'a> NodeDecoder<'a> {
    fn new(source: &'a [u8], bundle: &'a Bundle) -> Self {
        let mut starts = Vec::with_capacity(bundle.blocks.len());
        let mut offset = 0u64;
        for block in &bundle.blocks { starts.push(offset); offset += block.decoded as u64; }
        Self { source, bundle, starts, cached: None,
            decode_budget: MAX_TEXTURE_BUNDLE, copy_budget: 64 * 1024 * 1024 }
    }
    fn read(&mut self, start: u64, size: u64) -> io::Result<Vec<u8>> {
        super::cancelled()?;
        if start.checked_add(size).is_none_or(|end| end > self.bundle.decoded) {
            return Err(invalid("Unity selected-node range exceeds decoded bundle"));
        }
        self.copy_budget = self.copy_budget.checked_sub(size)
            .ok_or_else(|| crate::audit_io::budget_error("Unity selected-node aggregate copy budget exceeded"))?;
        let mut out = Vec::new();
        out.try_reserve_exact(size as usize).map_err(|_| invalid("Unity selected-node allocation failed"))?;
        if size == 0 { return Ok(out); }
        let mut index = self.starts.partition_point(|offset| *offset <= start) - 1;
        let mut position = start;
        while out.len() < size as usize {
            super::cancelled()?;
            let block = self.bundle.blocks.get(index).ok_or_else(|| invalid("Unity selected node lacks a decoded block"))?;
            if self.cached.as_ref().is_none_or(|(cached, _)| *cached != index) {
                self.decode_budget = self.decode_budget.checked_sub(block.decoded as u64)
                    .ok_or_else(|| crate::audit_io::budget_error("Unity selected-node block decode budget exceeded"))?;
                // Drop the old buffer before requesting another up-to-64 MiB block.
                self.cached = None;
                let encoded = self.source.get(block.offset..block.offset + block.encoded)
                    .ok_or_else(|| invalid("UnityFS selected block exceeds source"))?;
                let bytes = decode(encoded, block.decoded, (block.flags & 0x3f) as u32)?;
                self.cached = Some((index, bytes));
            }
            let bytes = &self.cached.as_ref().unwrap().1;
            let local = (position - self.starts[index]) as usize;
            let available = bytes.get(local..).ok_or_else(|| invalid("Unity selected-node block offset is invalid"))?;
            let length = available.len().min(size as usize - out.len());
            if length == 0 { return Err(invalid("Unity selected-node read made no progress")); }
            out.extend_from_slice(&available[..length]);
            position += length as u64;
            index += 1;
        }
        Ok(out)
    }
}

/// Development-only detailed bundle Texture2D storage inventory. This is a
/// separate command from both the original node summary and recompression audit;
/// no original output contract or installed asset writer is replaced.
pub fn texture_inventory(path: &Path) -> io::Result<()> {
    use std::collections::BTreeMap;
    use crate::audit_io::field;
    let bytes = read_source_limit(path, MAX_TEXTURE_BUNDLE)?;
    let bundle = parse(&bytes)?;
    if bundle.decoded > MAX_TEXTURE_BUNDLE || bundle.nodes.len() > MAX_TEXTURE_NODES {
        return Err(invalid("Unity texture inventory exceeds its 128 MiB/4096-node work limit"));
    }
    let mut names = BTreeMap::<Vec<u8>, usize>::new();
    let mut node_extents = Vec::new();
    for (index, node) in bundle.nodes.iter().enumerate() {
        if names.insert(node.path.clone(), index).is_some() {
            return Err(invalid("Unity texture inventory refuses ambiguous duplicate node names"));
        }
        if node.size != 0 { node_extents.push((node.offset, node.offset + node.size)); }
    }
    node_extents.sort_unstable();
    if node_extents.windows(2).any(|p| p[0].1 > p[1].0) {
        return Err(invalid("Unity texture inventory refuses aliased/overlapping bundle nodes"));
    }
    let mut decoder = NodeDecoder::new(&bytes, &bundle);
    let mut metadata_budget = 32u64 * 1024 * 1024;
    let mut object_budget = 64u64 * 1024 * 1024;
    let mut report_budget = 10000u64;
    let mut nodes = Vec::new();
    let mut opaque_nodes = Vec::new();
    let mut streams = Vec::new();
    let mut ranges = BTreeMap::<usize, Vec<(u64, u64)>>::new();
    let (mut total_textures, mut inspected, mut inline_bytes, mut declared_stream_bytes) = (0u64, 0u64, 0u64, 0u64);
    for (index, node) in bundle.nodes.iter().enumerate() {
        super::cancelled()?;
        // Directory flag 4 (1 << 2) designates SerializedFiles in this subset.
        // Names alone never turn a resource stream into a serialized object.
        if node.flags != 4 {
            opaque_nodes.push((index, if node.flags == 0 { "RESOURCE_PAYLOAD_NOT_SELECTED" }
                else { "UNKNOWN_DIRECTORY_OR_DELETED_NODE_FLAGS" }));
            continue;
        }
        if node.size > 64 * 1024 * 1024 {
            opaque_nodes.push((index, "SERIALIZED_NODE_COPY_BUDGET_SKIPPED"));
            continue;
        }
        let slice = match decoder.read(node.offset, node.size) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) if crate::audit_io::is_budget_error(&error) => {
                opaque_nodes.push((index, "NODE_DECODE_OR_COPY_BUDGET_SKIPPED")); continue;
            }
            Err(error) => return Err(error),
        };
        if !crate::unity_serialized::plausible(&slice) {
            opaque_nodes.push((index, "UNSUPPORTED_SERIALIZED_HEADER")); continue;
        }
        let inventory = match crate::unity_serialized::bundle_texture_inventory(&slice,
            &mut metadata_budget, &mut object_budget, &mut report_budget) {
            Ok(v) => v,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) => {
                let status = if crate::audit_io::is_budget_error(&error) { "METADATA_BUDGET_SKIPPED" }
                    else if error.kind() == io::ErrorKind::Unsupported { "UNSUPPORTED_METADATA_SCHEMA" }
                    else { "INVALID_OR_UNSUPPORTED_METADATA" };
                opaque_nodes.push((index, status)); continue;
            }
        };
        total_textures = total_textures.saturating_add(inventory.textures);
        inspected += inventory.details.len() as u64;
        for (id, fields) in &inventory.details {
            let texture = &fields.texture;
            inline_bytes = inline_bytes.saturating_add(texture.inline_bytes as u64);
            declared_stream_bytes = declared_stream_bytes.saturating_add(texture.stream_size);
            // Mip byte formulas only describe one ordinary 2D image. Cube/array/
            // volume or absent dimension/image-count metadata must remain opaque.
            let shape_known = fields.image_count == Some(1) && fields.dimension == Some(2);
            let expected = if shape_known { texture_bytes(texture)? } else { None };
            let actual = if texture.stream_size != 0 { texture.stream_size } else { texture.inline_bytes as u64 };
            let mut stream = TextureStream { node: index, id: *id, target: None,
                offset: texture.stream_offset, size: texture.stream_size, status: "INLINE_METADATA_ONLY",
                shape_status: if !shape_known { "IMAGE_DIMENSION_UNPROVEN" }
                    else if expected.is_none() { "CODEC_LAYOUT_UNKNOWN" }
                    else if expected == Some(actual) { "DECLARED_LENGTH_MATCHES_KNOWN_MIP_SHAPE" }
                    else { "DECLARED_LENGTH_DIFFERS_FROM_KNOWN_MIP_SHAPE" },
                expected, actual };
            if texture.stream_size != 0 {
                stream.status = "UNSUPPORTED_STREAM_NAMESPACE";
                if let Some(name) = stream_node(&node.path, &texture.stream_path) {
                    stream.status = "MISSING_SAME_BUNDLE_NODE";
                    if let Some(&target) = names.get(name) {
                        stream.target = Some(target);
                        let target_node = &bundle.nodes[target];
                        let end = texture.stream_offset.checked_add(texture.stream_size)
                            .ok_or_else(|| invalid("Unity stream extent overflow"))?;
                        if target_node.flags != 0 {
                            stream.status = "TARGET_NODE_KIND_UNSUPPORTED";
                        } else if end > target_node.size {
                            stream.status = "OUT_OF_RANGE";
                        } else {
                            stream.status = "BOUNDS_ONLY_OWNERSHIP_UNPROVEN";
                            ranges.entry(target).or_default().push((texture.stream_offset, end));
                        }
                    }
                }
            }
            streams.push(stream);
        }
        nodes.push(TextureNodeInventory { node: index, inventory });
    }
    // Long node names repeat in object/field records. A row-count cap alone
    // would permit hundreds of MiB of terminal output. This conservative upper
    // bound charges escaped text and a numeric/record allowance before emitting
    // anything; it is a report budget, not a claim about peak process memory.
    let mut report_size = 4096u64;
    let mut charge = |name: &[u8], text: usize, count: usize| -> io::Result<()> {
        let row = (name.len() as u64).saturating_add(text as u64).saturating_mul(3).saturating_add(256);
        report_size = report_size.saturating_add(row.saturating_mul(count as u64));
        if report_size > MAX_TEXTURE_REPORT {
            return Err(crate::audit_io::budget_error("Unity texture inventory report exceeds 8 MiB budget"));
        }
        Ok(())
    };
    for node in &bundle.nodes { charge(&node.path, 0, 1)?; }
    for (index, _) in &opaque_nodes { charge(&bundle.nodes[*index].path, 0, 1)?; }
    for item in &nodes {
        let name = &bundle.nodes[item.node].path;
        charge(name, 0, 3 + item.inventory.object_records.len() + item.inventory.opaque.len())?;
        for reference in &item.inventory.external_records {
            charge(name, reference.asset_path.len() + reference.path.len(), 1)?;
        }
        for (_, fields) in &item.inventory.details {
            charge(name, fields.texture.stream_path.len(), 2)?;
            for path in fields.spans.keys() { charge(name, path.len(), 1)?; }
        }
    }
    for stream in &streams {
        let target_length = stream.target.map(|i| bundle.nodes[i].path.len()).unwrap_or(0);
        charge(&bundle.nodes[stream.node].path, target_length, 2)?;
    }
    for index in ranges.keys() { charge(&bundle.nodes[*index].path, 0, 1)?; }
    let mut range_reports = Vec::new();
    let mut unique_stream_bytes = 0u64;
    for (index, extents) in &mut ranges {
        let summary = crate::audit_io::summarize_ranges(extents)?;
        unique_stream_bytes = unique_stream_bytes.checked_add(summary.union)
            .ok_or_else(|| invalid("Unity bundle stream-union total overflow"))?;
        range_reports.push((*index, extents.len(), summary));
    }
    super::cancelled()?;
    println!("UNITY_BUNDLE_TEXTURE_HEADER|{}|{}|{}|{}", bundle.version, bundle.nodes.len(), nodes.len(), bundle.decoded);
    for (index, node) in bundle.nodes.iter().enumerate() {
        println!("UNITY_BUNDLE_NODE|{index}|{}|{}|{}|{}", field(&node.path), node.offset, node.size, node.flags);
    }
    for (index, status) in &opaque_nodes {
        println!("UNITY_BUNDLE_OPAQUE_NODE|{}|{status}", field(&bundle.nodes[*index].path));
    }
    for item in &nodes {
        let name = field(&bundle.nodes[item.node].path);
        let v = &item.inventory;
        println!("UNITY_BUNDLE_TEXTURE_NODE|{name}|{}|{}|{}|{}|{}|{}|{}", v.version,
            u8::from(v.trees), v.textures, v.details.len(), v.opaque.len(), v.unretained, v.external);
        println!("UNITY_BUNDLE_REFERENCE_RISK|{name}|{}|{}|{}|REFERENCES_NOT_RESOLVED", v.sprites, v.atlases, v.external);
        println!("UNITY_BUNDLE_OBJECT_ENDIAN|{name}|{}", if v.big { "big" } else { "little" });
        for reference in &v.external_records {
            let guid: String = reference.guid.iter().map(|b| format!("{b:02X}")).collect();
            println!("UNITY_BUNDLE_EXTERNAL|{name}|{}|{}|{}|{}|{}|UNRESOLVED", reference.index,
                field(reference.asset_path.as_bytes()), guid, reference.kind, field(reference.path.as_bytes()));
        }
        for object in &v.object_records {
            println!("UNITY_BUNDLE_OBJECT_SPAN|{name}|{}|{}|{}|{}|{}|{}", object.id,
                object.type_index, object.start, object.size, object.table_start, object.table_end);
        }
        for (id, fields) in &v.details {
            let t = &fields.texture;
            println!("UNITY_BUNDLE_TEXTURE|{name}|{id}|{}|{}|{}|{}|{}|{}|{}|{}|{}", t.width,
                t.height, t.format, crate::unity_tree::format_name(t.format), t.mips,
                t.inline_bytes, t.stream_offset, t.stream_size, field(t.stream_path.as_bytes()));
            println!("UNITY_BUNDLE_TEXTURE_SHAPE|{name}|{id}|{}|{}|METADATA_ONLY",
                fields.image_count.map(|n| n.to_string()).unwrap_or_default(),
                fields.dimension.map(|n| n.to_string()).unwrap_or_default());
            for (path, span) in &fields.spans {
                let (payload_offset, payload_size) = span.payload.map(|(o, n)| (o.to_string(), n.to_string()))
                    .unwrap_or_default();
                println!("UNITY_BUNDLE_FIELD_SPAN|{name}|{id}|{}|{}|{}|{}|{payload_offset}|{payload_size}|OBJECT_RELATIVE",
                    field(path.as_bytes()), span.start, span.end, span.kind);
            }
        }
        for (id, status) in &v.opaque { println!("UNITY_BUNDLE_OPAQUE_TEXTURE|{name}|{id}|{status}"); }
    }
    for stream in &streams {
        let target = stream.target.map(|i| field(&bundle.nodes[i].path)).unwrap_or_default();
        println!("UNITY_BUNDLE_STREAM|{}|{}|{}|{}|{}|{}", field(&bundle.nodes[stream.node].path),
            stream.id, stream.status, target, stream.offset, stream.size);
        println!("UNITY_BUNDLE_MIP_SHAPE|{}|{}|{}|{}|{}|PIXELS_UNDECODED", field(&bundle.nodes[stream.node].path),
            stream.id, stream.shape_status, stream.expected.map(|n| n.to_string()).unwrap_or_default(), stream.actual);
    }
    for (index, count, summary) in &range_reports {
        println!("UNITY_BUNDLE_STREAM_RANGES|{}|{}|{}|{}|{}|OWNERSHIP_UNPROVEN", field(&bundle.nodes[*index].path),
            count, summary.union, summary.aliases, summary.overlap_groups);
    }
    println!("UNITY_BUNDLE_TEXTURE_TOTAL|{total_textures}|{inspected}|{inline_bytes}|{declared_stream_bytes}|{unique_stream_bytes}|METADATA_ONLY");
    println!("UNITY_BUNDLE_TEXTURE_WORK|{}|{}|SELECTED_SERIALIZED_NODES_ONLY",
        MAX_TEXTURE_BUNDLE - decoder.decode_budget, 64 * 1024 * 1024 - decoder.copy_budget);
    eprintln!("Development read-only Unity bundle texture inventory: known field/mip shapes and same-bundle stream bounds only. Pixel bytes are not decoded, Sprite/mesh/external ownership is unproven, and no writer or savings claim is enabled.");
    Ok(())
}

/// Audit performs all compression/verification in RAM, writing nothing.
/// Export is gated on logical gain / complete output bytes, not game size.
pub fn run(input: &Path, output: Option<&Path>, min_efficiency: f64) -> io::Result<()> {
    if !min_efficiency.is_finite() || !(0.0..=100.0).contains(&min_efficiency) {
        return Err(invalid("UnityFS write-efficiency percentage must be between 0 and 100"));
    }
    // Refuse existing destinations before spending CPU on an export.
    if let Some(path) = output {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "UnityFS destination already exists")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (), Err(e) => return Err(e),
        }
    }
    let start = std::time::Instant::now();
    let source = read_source(input)?;
    let (candidate, changed, bundle) = rebuild(&source)?;
    let gain = source.len().saturating_sub(candidate.len());
    let efficiency = gain as f64 * 100.0 / candidate.len().max(1) as f64;
    let status = if gain == 0 { "NO_GAIN" } else if efficiency < min_efficiency { "LOW_EFFICIENCY" } else if let Some(path) = output {
        super::cancelled()?;
        let mut target = fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
        let result = (|| {
            for chunk in candidate.chunks(65536) { super::cancelled()?; target.write_all(chunk)?; }
            super::cancelled()?; target.sync_all()
        })();
        if let Err(e) = result { let _ = fs::remove_file(path); return Err(e); }
        "EXPORTED"
    } else { "CANDIDATE" };
    println!("UNITYFS|{}|{}|{}|{}|{}|{}|{:.4}|{:.6}|{}", source.len(), candidate.len(), bundle.decoded,
        bundle.blocks.len(), changed, bundle.nodes.len(), efficiency, start.elapsed().as_secs_f64(), status);
    eprintln!("Lossless UnityFS analysis: source unchanged. Logical savings only; physical savings and game/catalog CRC compatibility require separate validation.");
    Ok(())
}

// ---------------------------------------------------------------------------
// In-place Unity bundle texture apply
// ---------------------------------------------------------------------------
//
// The apply route is the M1 "export-first -> beta apply" step: instead of
// publishing a detached candidate bundle, it rewrites a bundle INSIDE a game
// copy in place, with the same restorable-backup, checksum-sidecar and
// atomic-write conventions the loose asset pipeline uses
// (`assets restore`/`assets finalize` handle the result unchanged).
//
// Integrity rule implemented here (and reported for every refusal):
//   * the bundle's own 16-byte info-section uncompressed-data hash must be all
//     zero; a nonzero value cannot be recomputed by this build, so the bundle
//     is refused (the zero placeholder is preserved byte-for-byte);
//   * a bundle whose game-relative path has an "aa" component (the Addressables
//     content layout) or whose directory/ancestor inside the game root contains
//     a `catalog.json`/`catalog.*.hash` file is refused because an Addressables
//     catalogue may carry a bundle hash/CRC this build cannot recompute.
// This is a layout rule, not universal proof: a catalogue stored elsewhere (or
// a custom content directory) is not detectable here at all, which is stated
// explicitly rather than guessed at.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) mod apply {
    use super::*;
    use crate::audit_io::{field, Source};
    use crate::texture_policy;
    use image::imageops::FilterType;
    use image_dds::{ImageFormat, Mipmaps, Quality, Surface as DdsSurface, SurfaceRgba8};
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, PathBuf};

    /// The apply route decodes whole SerializedFile nodes per texture and then
    /// verifies every node again; these budgets are deliberately far above a
    /// single read-only audit's so a many-texture node stays workable, and they
    /// still fail closed rather than looping.
    const APPLY_DECODE_BUDGET: u64 = 16 * 1024 * 1024 * 1024;
    const APPLY_COPY_BUDGET: u64 = 16 * 1024 * 1024 * 1024;
    const APPLY_READ_BUDGET: u64 = MAX_TEXTURE_BUNDLE + 64 * 1024 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum ApplyStatus { Applied, NoGain, AlreadyOptimized, Refused }

    #[derive(Debug, Clone)]
    pub(crate) struct AppliedTexture {
        pub node_index: usize,
        pub path_id: u64,
        pub old_width: u32,
        pub old_height: u32,
        pub old_mips: u32,
        pub new_width: u32,
        pub new_height: u32,
        pub new_mips: u32,
        pub old_len: u64,
        pub new_len: u64,
        pub format: u32,
        pub storage: &'static str,
        pub mode: &'static str,
    }

    #[derive(Debug)]
    pub(crate) struct ApplyOutcome {
        pub relative: PathBuf,
        pub status: ApplyStatus,
        pub refusal: Option<String>,
        pub applied: usize,
        pub skipped: usize,
        pub old_bytes: u64,
        pub new_bytes: u64,
        pub removed_decoded: u64,
        pub skips: Vec<String>,
        pub textures: Vec<AppliedTexture>,
    }

    #[derive(Debug, Clone)]
    struct OriginalTexture {
        node_index: usize,
        path_id: u64,
        inline: Option<(usize, usize)>,
        streamed: Option<(usize, u64, u64)>,
    }

    /// One streamed texture reference in the source bundle whose declared
    /// `m_StreamData/offset` may need a shifted value in the final layout.
    #[derive(Debug, Clone)]
    struct StreamedRef {
        node_index: usize,
        target: usize,
        offset: u64,
        field: Option<(usize, bool)>,
    }

    struct Job {
        node_index: usize,
        path_id: u64,
        node_offset: u64,
        format: ImageFormat,
        format_id: u32,
        patches: Vec<NodePatch>,
        old_payload: Vec<u8>,
        new_payload: Vec<u8>,
        old_width: u32,
        old_height: u32,
        old_mips: u32,
        new_width: u32,
        new_height: u32,
        new_mips: u32,
        target: usize,
        orig_stream_offset: u64,
        orig_stream_size: u64,
        inline: bool,
        compaction: bool,
    }

    /// The location of the 8-byte `m_StreamData/offset` field, if the schema
    /// captured a span that holds exactly the parsed offset. `None` means the
    /// reference cannot be shifted (which blocks physical compaction already).
    /// Built from the shared inventory so even textures the apply skips are
    /// tracked when a compaction shifts them.
    fn streamed_offset_field(fields: &crate::unity_tree::TextureFields, slice: &[u8],
        record: &crate::unity_serialized::TextureObjectRecord, big: bool) -> Option<(usize, bool)> {
        let span = fields.spans.get("m_StreamData/offset")?;
        if span.kind != "PRIMITIVE_BITS" || span.end != span.start + 8 { return None; }
        let at = record.start as usize + span.start;
        if read_u64(slice, at, big).ok() != Some(fields.texture.stream_offset) { return None; }
        Some((at, big))
    }

    /// The stated rule for a bundle this build cannot apply safely. `Some` is a
    /// refusal reason, never a guess; `None` means the in-bundle hash is the
    /// zero placeholder and no Addressables marker was found in the layout.
    fn integrity_refusal(root: &Path, relative: &Path, bundle: &Bundle) -> Option<String> {
        if bundle.info[..16].iter().any(|byte| *byte != 0) {
            return Some(format!(
                "{}: the 16-byte info-section uncompressed-data hash is nonzero, its algorithm is unproven and it cannot be recomputed",
                crate::assets::ASSET_REFUSAL_HASH));
        }
        if relative.components().any(|component| matches!(component,
            Component::Normal(name) if name.eq_ignore_ascii_case("aa")))
        {
            return Some(format!(
                "{}: a game-relative path component is 'aa' (Addressables content layout); a catalogue may carry a bundle hash/CRC this build cannot recompute",
                crate::assets::ASSET_REFUSAL_CATALOGUE));
        }
        let mut directory = relative.parent();
        while let Some(current) = directory {
            for name in ["catalog.json", "catalog.hash", "catalog.json.hash"] {
                let candidate = if current.as_os_str().is_empty() {
                    root.join(name)
                } else {
                    root.join(current).join(name)
                };
                if candidate.exists() {
                    return Some(format!(
                        "{}: Addressables catalogue file {} exists beside this bundle and may carry a hash/CRC this build cannot recompute",
                        crate::assets::ASSET_REFUSAL_CATALOGUE, candidate.display()));
                }
            }
            directory = current.parent();
        }
        None
    }

    /// Decode, downscale to the repository policy edge and re-encode one mip
    /// chain with the same path `texture-compress` uses: `Surface::decode_rgba8`,
    /// Lanczos3 resampling and the same codec with a regenerated complete
    /// 1x1-terminated chain, verified by the shared DDS parser and (for the new
    /// chain) `prove_shrink_chain`. Returns `None` when no strictly smaller
    /// payload exists, and refuses a result the policy or the block floor reject.
    fn downscale_payload(format: ImageFormat, width: u32, height: u32, mips: u32,
        payload: &[u8], edge: u32) -> io::Result<Option<(Vec<u8>, u32, u32, u32)>> {
        if width == 0 || height == 0 || mips == 0 {
            return Err(invalid("Unity texture shape cannot be decoded"));
        }
        let surface = DdsSurface { width, height, depth: 1, layers: 1, mipmaps: mips,
            image_format: format, data: payload };
        let decoded = surface.decode_rgba8()
            .map_err(|error| invalid(&format!("Unity texture decode failed: {error}")))?;
        let base = decoded.get_image(0, 0, 0).ok_or_else(|| invalid("Unity texture has no base mip"))?;
        let (w, h) = base.dimensions();
        let (new_width, new_height) = crate::texture::scale_to(w, h, edge);
        if (new_width, new_height) == (w, h) { return Ok(None); }
        if texture_policy::output_too_thin(new_width, new_height) {
            return Err(invalid(&format!(
                "Unity texture downscale to {new_width}x{new_height} is refused by the repository texture policy (short edge below {})",
                texture_policy::MIN_SHORT_EDGE)));
        }
        if (w >= 4 && new_width < 4) || (h >= 4 && new_height < 4) {
            return Err(invalid("Unity texture downscale below the 4x4 block floor is not supported"));
        }
        let resized = image::imageops::resize(&base, new_width, new_height, FilterType::Lanczos3);
        let encoded = SurfaceRgba8::from_image(&resized)
            .encode(format, Quality::Normal, Mipmaps::GeneratedAutomatic)
            .map_err(|error| invalid(&format!("Unity texture encode failed: {error}")))?;
        let expected_mips = full_mip_levels(new_width, new_height);
        if encoded.mipmaps != expected_mips {
            return Err(invalid("Unity texture encode did not produce the complete 1x1-terminated mip chain"));
        }
        let dds = crate::texture::build_dds(&encoded)?;
        let check = crate::texture::parse(&dds)?;
        if check.width != encoded.width || check.height != encoded.height
            || check.mipmaps != encoded.mipmaps || check.format != encoded.image_format {
            return Err(invalid("Unity texture shrink payload failed its container self-check"));
        }
        let new_payload = dds[check.data_start..].to_vec();
        prove_shrink_chain(format, new_width, new_height, expected_mips, &new_payload)?;
        if new_payload.len() >= payload.len() { return Ok(None); }
        Ok(Some((new_payload, new_width, new_height, expected_mips)))
    }

    /// Select, encode and rebuild one bundle entirely in memory; the caller
    /// holds the mutation lock and gets bytes that were re-proven against the
    /// source before anything is installed.
    fn apply_one(tree: &crate::Tree, profile_edge: u32, level: u8, root: &Path, relative: &Path)
        -> io::Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome {
            relative: relative.to_path_buf(), status: ApplyStatus::NoGain, refusal: None,
            applied: 0, skipped: 0, old_bytes: 0, new_bytes: 0, removed_decoded: 0,
            skips: Vec::new(), textures: Vec::new(),
        };
        if relative.as_os_str().is_empty()
            || relative.components().any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(invalid("Unity bundle apply requires a non-empty game-relative path"));
        }
        let root_dev = fs::symlink_metadata(root)?.dev();
        let path = root.join(relative);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() || meta.nlink() != 1 || meta.dev() != root_dev {
            return Err(invalid("Unity bundle apply requires a regular single-link game file"));
        }
        outcome.old_bytes = meta.len();
        if meta.len() > MAX_TEXTURE_BUNDLE {
            outcome.status = ApplyStatus::Refused;
            outcome.refusal = Some(format!("bundle is {} bytes, above the {} MiB Unity texture apply work limit",
                meta.len(), MAX_TEXTURE_BUNDLE / (1024 * 1024)));
            return Ok(outcome);
        }
        if crate::assets::backup_path(root, relative).exists() {
            outcome.status = ApplyStatus::AlreadyOptimized;
            return Ok(outcome);
        }
        let mut source = Source::open(&path, MAX_TEXTURE_BUNDLE, APPLY_READ_BUDGET)?;
        let bytes = source.read_at(0, source.len() as usize)?;
        let bundle = parse(&bytes)?;
        if bundle.decoded > MAX_TEXTURE_BUNDLE || bundle.nodes.len() > MAX_TEXTURE_NODES {
            outcome.status = ApplyStatus::Refused;
            outcome.refusal = Some(format!("bundle exceeds the {} MiB/{} node Unity texture apply work limit",
                MAX_TEXTURE_BUNDLE / (1024 * 1024), MAX_TEXTURE_NODES));
            return Ok(outcome);
        }
        if let Some(reason) = integrity_refusal(root, relative, &bundle) {
            outcome.status = ApplyStatus::Refused;
            outcome.refusal = Some(reason);
            return Ok(outcome);
        }

        // ---- selection ---------------------------------------------------
        let mut decoder = NodeDecoder::new(&bytes, &bundle);
        decoder.decode_budget = APPLY_DECODE_BUDGET;
        decoder.copy_budget = APPLY_COPY_BUDGET;
        let mut jobs: Vec<Job> = Vec::new();
        let mut refs: Vec<StreamedRef> = Vec::new();
        let mut originals: Vec<OriginalTexture> = Vec::new();
        let mut original_slices: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        let mut skips: Vec<String> = Vec::new();
        for (node_index, node) in bundle.nodes.iter().enumerate() {
            crate::cancelled()?;
            if node.flags != 4 { continue; }
            let name = String::from_utf8_lossy(&node.path).into_owned();
            if node.size > 64 * 1024 * 1024 {
                skips.push(format!("node {node_index} ({name}): SerializedFile exceeds the 64 MiB node limit; no texture in it can be applied"));
                continue;
            }
            let node_slice = match decoder.read(node.offset, node.size) {
                Ok(slice) => slice,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(error) if crate::audit_io::is_budget_error(&error) => {
                    skips.push(format!("node {node_index} ({name}): decode budget exceeded; refusing to guess"));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !crate::unity_serialized::plausible(&node_slice) {
                skips.push(format!("node {node_index} ({name}): not a supported SerializedFile header; no texture in it can be applied"));
                continue;
            }
            let inventory = match node_texture_inventory(&node_slice) {
                Ok(inventory) => inventory,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(error) if crate::audit_io::is_budget_error(&error) => {
                    skips.push(format!("node {node_index} ({name}): texture inventory budget exceeded; refusing to guess"));
                    continue;
                }
                Err(error) => {
                    skips.push(format!("node {node_index} ({name}): texture inventory failed ({error}); no texture can be proven"));
                    continue;
                }
            };
            original_slices.insert(node_index, node_slice.clone());
            for (id, fields) in &inventory.details {
                crate::cancelled()?;
                let texture = &fields.texture;
                // Record how to re-extract this texture from the source even
                // when it is skipped below: the final verification compares
                // every other payload byte-for-byte, not just the applied ones.
                // A resolved stream target also yields the offset reference the
                // global verification needs when a compaction shifts it, even
                // for a texture that is not applied itself.
                let resolved_target = if texture.stream_size != 0 {
                    stream_node(&node.path, &texture.stream_path)
                        .and_then(|name| bundle.nodes.iter().position(|candidate| candidate.path == name))
                } else { None };
                {
                    let inline = if texture.inline_bytes != 0 {
                        fields.spans.get("image data")
                            .or_else(|| fields.spans.get("image data/Array"))
                            .and_then(|span| span.payload.filter(|(_, len)| *len == texture.inline_bytes))
                            .and_then(|(offset, len)| inventory.object_records.iter().find(|record| record.id == *id)
                                .map(|record| (record.start as usize + offset, len)))
                    } else { None };
                    let streamed = resolved_target.map(|target| (target, texture.stream_offset, texture.stream_size));
                    if inline.is_some() || streamed.is_some() {
                        originals.push(OriginalTexture { node_index, path_id: *id, inline, streamed });
                    }
                }
                if let Some(target) = resolved_target {
                    let field = inventory.object_records.iter().find(|record| record.id == *id)
                        .and_then(|record| streamed_offset_field(fields, &node_slice, record, inventory.big));
                    refs.push(StreamedRef { node_index, target, offset: texture.stream_offset, field });
                }
                if fields.image_count != Some(1) || fields.dimension != Some(2) {
                    skips.push(format!("node {node_index} object {id}: not an ordinary 2D image (m_ImageCount/m_TextureDimension)"));
                    continue;
                }
                let Some(expected) = texture_bytes(texture)? else {
                    skips.push(format!("node {node_index} object {id}: format {} ({}) is outside the apply subset",
                        texture.format, crate::unity_tree::format_name(texture.format)));
                    continue;
                };
                let actual = if texture.stream_size != 0 { texture.stream_size } else { texture.inline_bytes as u64 };
                if expected != actual {
                    skips.push(format!("node {node_index} object {id}: declared payload length {actual} disagrees with its known mip shape {expected}"));
                    continue;
                }
                if texture.inline_bytes != 0 && texture.stream_size != 0 {
                    skips.push(format!("node {node_index} object {id}: declares both inline and streamed storage"));
                    continue;
                }
                let edge = texture_policy::effective_edge(profile_edge, texture.width, texture.height);
                if texture.width.max(texture.height) <= edge {
                    skips.push(format!("node {node_index} object {id}: {}x{} is already within the active cap {edge}; no downscale",
                        texture.width, texture.height));
                    continue;
                }
                let located = match locate_texture_with_slice(node_index, *id, &bundle, &mut decoder, Some(&node_slice)) {
                    Ok(located) => located,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                    Err(error) => {
                        skips.push(format!("node {node_index} object {id}: {error}"));
                        continue;
                    }
                };
                let old_payload = match &located.storage {
                    LocatedStorage::Inline { payload_start, payload_len, .. } => {
                        located.slice[*payload_start..*payload_start + *payload_len].to_vec()
                    }
                    LocatedStorage::Streamed { target, offset, size } => {
                        let target_node = &bundle.nodes[*target];
                        decoder.read(target_node.offset + *offset, *size)?
                    }
                };
                let scaled = match downscale_payload(located.format, texture.width, texture.height,
                    texture.mips, &old_payload, edge) {
                    Ok(Some(scaled)) => scaled,
                    Ok(None) => {
                        skips.push(format!("node {node_index} object {id}: no strictly smaller re-encode at cap {edge}"));
                        continue;
                    }
                    Err(error) if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::Unsupported) => {
                        skips.push(format!("node {node_index} object {id}: {error}"));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let (new_payload, new_width, new_height, new_mips) = scaled;
                let patches = match &located.storage {
                    LocatedStorage::Inline { .. } => inline_shrink_patches(&located, &new_payload, new_width, new_height, new_mips),
                    LocatedStorage::Streamed { .. } => streamed_shrink_patches(&located, new_payload.len(), new_width, new_height, new_mips),
                };
                let patches = match patches {
                    Ok(patches) => patches,
                    Err(error) if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::Unsupported) => {
                        skips.push(format!("node {node_index} object {id}: {error}"));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let (target, start, size, inline) = match &located.storage {
                    LocatedStorage::Inline { .. } => (node_index, 0u64, 0u64, true),
                    LocatedStorage::Streamed { target, offset, size } => (*target, *offset, *size, false),
                };
                jobs.push(Job { node_index, path_id: *id, node_offset: located.node_offset,
                    format: located.format, format_id: texture.format, patches, old_payload,
                    new_payload, old_width: texture.width, old_height: texture.height,
                    old_mips: texture.mips, new_width, new_height, new_mips,
                    target, orig_stream_offset: start, orig_stream_size: size, inline, compaction: false });
            }
        }
        outcome.skipped = skips.len();
        outcome.skips = skips;
        if jobs.is_empty() {
            return Ok(outcome);
        }

        // ---- rebuild in memory -------------------------------------------
        let mut working = bytes.clone();
        let inline_jobs: Vec<usize> = jobs.iter().enumerate().filter(|(_, job)| job.inline).map(|(i, _)| i).collect();
        if !inline_jobs.is_empty() {
            let mut patches: Vec<(usize, Vec<u8>)> = Vec::new();
            for &index in &inline_jobs {
                let job = &jobs[index];
                for patch in &job.patches {
                    patches.push((job.node_offset as usize + patch.start, patch.bytes.clone()));
                }
            }
            patches.sort_unstable_by_key(|(start, _)| *start);
            for pair in patches.windows(2) {
                if pair[0].0 + pair[0].1.len() > pair[1].0 {
                    return Err(invalid("Unity bundle apply inline patches overlap"));
                }
            }
            let refs_vec: Vec<(usize, &[u8])> = patches.iter().map(|(start, bytes)| (*start, bytes.as_slice())).collect();
            let (candidate, _changed, _) = rebuild_patches(&working, &refs_vec)?;
            working = candidate;
        }
        let mut rejected: Vec<usize> = Vec::new();
        for index in 0..jobs.len() {
            if jobs[index].inline { continue; }
            crate::cancelled()?;
            let current_bundle = parse(&working)?;
            let mut current_decoder = NodeDecoder::new(&working, &current_bundle);
            current_decoder.decode_budget = APPLY_DECODE_BUDGET;
            current_decoder.copy_budget = APPLY_COPY_BUDGET;
            let located_now = locate_texture(jobs[index].node_index, jobs[index].path_id, &current_bundle, &mut current_decoder)?;
            match streamed_shrink_candidate(&working, &current_bundle, &located_now,
                jobs[index].node_index, jobs[index].path_id, &jobs[index].new_payload,
                jobs[index].new_width, jobs[index].new_height, jobs[index].new_mips) {
                Ok(result) => {
                    jobs[index].compaction = result.mode == StreamedShrinkMode::Compacted;
                    working = result.candidate;
                }
                Err(error) if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::Unsupported) => {
                    rejected.push(index);
                    outcome.skips.push(format!("node {} object {}: {error}", jobs[index].node_index, jobs[index].path_id));
                }
                Err(error) => return Err(error),
            }
        }
        let applied: Vec<&Job> = jobs.iter().enumerate()
            .filter(|(index, _)| !rejected.contains(index)).map(|(_, job)| job).collect();
        if applied.is_empty() || working.len() >= bytes.len() {
            outcome.skipped += rejected.len();
            return Ok(outcome);
        }

        // ---- full pre-install verification against the source ------------
        let check_bundle = parse(&working)?;
        verify_applied_bundle(&bytes, &bundle, &working, &check_bundle, &applied,
            &refs, &originals, &original_slices)?;
        source.unchanged()?;

        // ---- install with the loose-asset safety sequence ----------------
        let (_, installed_len) = crate::assets::install_verified_asset(tree, root, relative, &working, level)?;
        let readback = fs::read(&path)?;
        if readback != working {
            return Err(invalid("installed Unity bundle differs from the verified candidate"));
        }
        let backup = crate::assets::backup_path(root, relative);
        if fs::read(&backup)? != bytes {
            return Err(invalid("restorable copy differs from the source bundle"));
        }
        if !crate::assets::matches_recorded_checksum(root, relative)? {
            return Err(invalid("installed Unity bundle does not match its recorded checksum"));
        }
        let installed_bundle = parse(&readback)?;
        verify_installed_payloads(&readback, &installed_bundle, &applied)?;

        outcome.status = ApplyStatus::Applied;
        outcome.applied = applied.len();
        outcome.skipped += rejected.len();
        outcome.new_bytes = installed_len;
        outcome.removed_decoded = applied.iter()
            .filter(|job| !job.inline && job.compaction).map(|job| job.orig_stream_size - job.new_payload.len() as u64).sum();
        outcome.textures = applied.iter().map(|job| AppliedTexture {
            node_index: job.node_index, path_id: job.path_id,
            old_width: job.old_width, old_height: job.old_height, old_mips: job.old_mips,
            new_width: job.new_width, new_height: job.new_height, new_mips: job.new_mips,
            old_len: job.old_payload.len() as u64, new_len: job.new_payload.len() as u64,
            format: job.format_id,
            storage: if job.inline { "inline" } else { "streamed" },
            mode: if job.inline { "inline-shrink" } else if job.compaction { "stream-compacted" } else { "stream-zero-padded" },
        }).collect();
        Ok(outcome)
    }

    /// The expected total shift of one streamed reference given the physical
    /// removals in its target node (original node-relative coordinates). A
    /// removal shifts a reference whose declared range starts at or after the
    /// removed extent's end.
    fn shifted_offset(reference: &StreamedRef, removals: &BTreeMap<usize, Vec<(u64, u64)>>) -> io::Result<u64> {
        let mut shift = 0u64;
        if let Some(ranges) = removals.get(&reference.target) {
            for (start, end) in ranges {
                if reference.offset >= *end {
                    shift = shift.checked_add(end - start)
                        .ok_or_else(|| invalid("Unity bundle apply shift total overflows"))?;
                } else if reference.offset >= *start {
                    return Err(invalid("a streamed extent lies inside another removal and cannot be proven shiftable"));
                }
            }
        }
        reference.offset.checked_sub(shift)
            .ok_or_else(|| invalid("Unity bundle apply stream offset would underflow"))
    }

    /// Reconstruct one resource node's expected content from the source: each
    /// declared extent keeps its new payload head; a physically compacted
    /// extent drops the freed tail, a zero-padded extent keeps its old size
    /// with a zero tail.
    fn expected_stream_node(original: &[u8], jobs: &[&Job], node_index: usize) -> io::Result<Vec<u8>> {
        let mut operations: Vec<&Job> = jobs.iter().copied()
            .filter(|job| !job.inline && job.target == node_index).collect();
        operations.sort_unstable_by_key(|job| job.orig_stream_offset);
        let mut out = Vec::with_capacity(original.len());
        let mut position = 0u64;
        for job in operations {
            let start = job.orig_stream_offset;
            let end = start.checked_add(job.orig_stream_size)
                .ok_or_else(|| invalid("Unity streamed extent overflow"))?;
            if start < position || end > original.len() as u64 {
                return Err(invalid("Unity streamed extents overlap or exceed their node"));
            }
            if job.new_payload.len() as u64 >= job.orig_stream_size {
                return Err(invalid("Unity streamed replacement is not strictly shorter"));
            }
            out.extend_from_slice(&original[position as usize..start as usize]);
            out.extend_from_slice(&job.new_payload);
            if !job.compaction {
                out.resize(out.len() + (job.orig_stream_size - job.new_payload.len() as u64) as usize, 0);
            }
            position = end;
        }
        out.extend_from_slice(&original[position as usize..]);
        Ok(out)
    }

    /// Prove the final in-memory candidate against the source with one global
    /// model: the node table may only shrink by the physically removed stream
    /// bytes and shift later nodes down by exactly that; every SerializedFile
    /// node changes only through the declared per-texture field/payload patches
    /// (plus offset shifts the compaction itself proved); every resource node
    /// matches the composed payload/tail expectation; and every retained
    /// texture payload re-extracts byte-identically except the applied ones,
    /// which must equal their new payload and decode at their new dimensions.
    fn verify_applied_bundle(bytes: &[u8], bundle: &Bundle, candidate: &[u8], check_bundle: &Bundle,
        jobs: &[&Job], refs: &[StreamedRef], originals: &[OriginalTexture],
        original_slices: &BTreeMap<usize, Vec<u8>>) -> io::Result<()> {
        if check_bundle.flags != bundle.flags || check_bundle.info[..16] != bundle.info[..16]
            || check_bundle.nodes.len() != bundle.nodes.len() {
            return Err(invalid("Unity bundle apply changed the container identity"));
        }
        // Physical removals per resource node, in original node-relative bytes.
        let mut removals: BTreeMap<usize, Vec<(u64, u64)>> = BTreeMap::new();
        for job in jobs {
            if job.inline || !job.compaction { continue; }
            removals.entry(job.target).or_default()
                .push((job.orig_stream_offset + job.new_payload.len() as u64,
                       job.orig_stream_offset + job.orig_stream_size));
        }
        let removed_in = |node: usize| -> u64 {
            removals.get(&node).map(|ranges| ranges.iter().map(|(start, end)| end - start).sum()).unwrap_or(0)
        };
        let total_removed: u64 = bundle.nodes.iter().enumerate().map(|(index, _)| removed_in(index)).sum();
        let mut prefix = 0u64;
        for (index, (old, new)) in bundle.nodes.iter().zip(&check_bundle.nodes).enumerate() {
            if old.flags != new.flags || old.path != new.path {
                return Err(invalid("Unity bundle apply changed a bundle node entry"));
            }
            if new.offset != old.offset - prefix || new.size + removed_in(index) != old.size {
                return Err(invalid("Unity bundle apply shifted the node table unexpectedly"));
            }
            prefix += removed_in(index);
        }
        if check_bundle.decoded + total_removed != bundle.decoded {
            return Err(invalid("Unity bundle apply removed an unexpected number of decoded bytes"));
        }
        let mut old_decoder = NodeDecoder::new(bytes, bundle);
        let mut new_decoder = NodeDecoder::new(candidate, check_bundle);
        for decoder in [&mut old_decoder, &mut new_decoder] {
            decoder.decode_budget = APPLY_DECODE_BUDGET;
            decoder.copy_budget = APPLY_COPY_BUDGET;
        }
        let has_stream_job = |node: usize| jobs.iter().any(|job| !job.inline && job.target == node);
        for (index, old_node) in bundle.nodes.iter().enumerate() {
            crate::cancelled()?;
            let new_node = &check_bundle.nodes[index];
            let original = match original_slices.get(&index) {
                Some(slice) => slice.clone(),
                None => old_decoder.read(old_node.offset, old_node.size)?,
            };
            let expected = if has_stream_job(index) {
                expected_stream_node(&original, jobs, index)?
            } else {
                let mut expected = original;
                let mut node_patches: Vec<&NodePatch> = jobs.iter()
                    .filter(|job| job.node_index == index)
                    .flat_map(|job| job.patches.iter()).collect();
                node_patches.sort_unstable_by_key(|patch| patch.start);
                for pair in node_patches.windows(2) {
                    if pair[0].end > pair[1].start {
                        return Err(invalid("Unity bundle apply node patches overlap"));
                    }
                }
                for patch in node_patches {
                    if patch.end > expected.len() {
                        return Err(invalid("Unity bundle apply patch exceeds its SerializedFile node"));
                    }
                    expected[patch.start..patch.end].copy_from_slice(&patch.bytes);
                }
                for reference in refs.iter().filter(|reference| reference.node_index == index) {
                    let shifted = shifted_offset(reference, &removals)?;
                    if shifted == reference.offset { continue; }
                    let Some((at, big)) = reference.field else {
                        return Err(invalid("a shifted stream offset has no proven field span"));
                    };
                    let end = at + 8;
                    if end > expected.len() {
                        return Err(invalid("a shifted stream offset field exceeds its SerializedFile node"));
                    }
                    if read_u64(&expected, at, big)? != reference.offset {
                        return Err(invalid("a stream offset field does not hold the parsed offset"));
                    }
                    let value = if big { shifted.to_be_bytes() } else { shifted.to_le_bytes() };
                    expected[at..end].copy_from_slice(&value);
                }
                expected
            };
            let actual = new_decoder.read(new_node.offset, new_node.size)?;
            if actual != expected {
                return Err(invalid(&format!("Unity bundle apply changed node {index} outside its declared patches")));
            }
        }
        // Every retained texture payload in the source must re-extract from the
        // candidate: applied ones equal their new payload and decode at their
        // new dimensions; all others stay byte-identical.
        let mut final_slices: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        let mut check_decoder = NodeDecoder::new(candidate, check_bundle);
        check_decoder.decode_budget = APPLY_DECODE_BUDGET;
        check_decoder.copy_budget = APPLY_COPY_BUDGET;
        for recipe in originals {
            crate::cancelled()?;
            if !final_slices.contains_key(&recipe.node_index) {
                let node = &check_bundle.nodes[recipe.node_index];
                final_slices.insert(recipe.node_index, check_decoder.read(node.offset, node.size)?);
            }
            let final_slice = &final_slices[&recipe.node_index];
            let located = match locate_texture_with_slice(recipe.node_index, recipe.path_id,
                check_bundle, &mut check_decoder, Some(final_slice)) {
                Ok(located) => located,
                Err(error) if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::Unsupported) => {
                    // The node bytes were already compared; a texture the shared
                    // selection cannot re-locate cannot be claimed either way.
                    continue;
                }
                Err(error) => return Err(error),
            };
            let final_payload = match &located.storage {
                LocatedStorage::Inline { payload_start, payload_len, .. } =>
                    located.slice[*payload_start..*payload_start + *payload_len].to_vec(),
                LocatedStorage::Streamed { target, offset, size } => {
                    let node = &check_bundle.nodes[*target];
                    check_decoder.read(node.offset + *offset, *size)?
                }
            };
            let applied = jobs.iter().find(|job| job.node_index == recipe.node_index && job.path_id == recipe.path_id);
            match applied {
                Some(job) => {
                    let texture = &located.fields.texture;
                    if final_payload != job.new_payload
                        || texture.width != job.new_width || texture.height != job.new_height
                        || texture.mips != job.new_mips || texture.format != job.format_id {
                        return Err(invalid("Unity bundle apply target did not re-extract at its new declared shape"));
                    }
                    let surface = DdsSurface { width: job.new_width, height: job.new_height,
                        depth: 1, layers: 1, mipmaps: job.new_mips,
                        image_format: job.format, data: &final_payload };
                    let decoded = surface.decode_rgba8()
                        .map_err(|error| invalid(&format!("Unity bundle apply target decode failed: {error}")))?;
                    if decoded.width != job.new_width || decoded.height != job.new_height
                        || decoded.depth != 1 || decoded.layers != 1 {
                        return Err(invalid("Unity bundle apply target decoded to an unexpected shape"));
                    }
                }
                None => {
                    let original_payload = match (recipe.inline, recipe.streamed) {
                        (Some((start, len)), _) => {
                            let slice = original_slices.get(&recipe.node_index)
                                .ok_or_else(|| invalid("Unity bundle apply lost an original SerializedFile slice"))?;
                            slice[start..start + len].to_vec()
                        }
                        (None, Some((target, offset, size))) => {
                            let node = &bundle.nodes[target];
                            old_decoder.read(node.offset + offset, size)?
                        }
                        (None, None) => return Err(invalid("Unity bundle apply lost an original texture location")),
                    };
                    if final_payload != original_payload {
                        return Err(invalid(&format!("Unity bundle apply changed an unrelated texture payload (node {} object {})",
                            recipe.node_index, recipe.path_id)));
                    }
                }
            }
        }
        Ok(())
    }

    /// Cheap post-install check on the bytes actually on disk: they re-parse and
    /// every applied texture still re-extracts with its new declared shape.
    fn verify_installed_payloads(installed: &[u8], bundle: &Bundle, jobs: &[&Job]) -> io::Result<()> {
        let mut decoder = NodeDecoder::new(installed, bundle);
        decoder.decode_budget = APPLY_DECODE_BUDGET;
        decoder.copy_budget = APPLY_COPY_BUDGET;
        let mut slices: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        for job in jobs {
            if !slices.contains_key(&job.node_index) {
                let node = &bundle.nodes[job.node_index];
                slices.insert(job.node_index, decoder.read(node.offset, node.size)?);
            }
            let slice = &slices[&job.node_index];
            let located = locate_texture_with_slice(job.node_index, job.path_id, bundle, &mut decoder, Some(slice))?;
            if located.fields.texture.width != job.new_width || located.fields.texture.height != job.new_height
                || located.fields.texture.mips != job.new_mips {
                return Err(invalid("the installed Unity bundle does not re-parse the applied texture shape"));
            }
            let payload = match &located.storage {
                LocatedStorage::Inline { payload_start, payload_len, .. } =>
                    located.slice[*payload_start..*payload_start + *payload_len].to_vec(),
                LocatedStorage::Streamed { target, offset, size } => {
                    let node = &bundle.nodes[*target];
                    decoder.read(node.offset + *offset, *size)?
                }
            };
            if payload != job.new_payload {
                return Err(invalid("the installed Unity bundle does not re-extract the applied payload"));
            }
        }
        Ok(())
    }

    /// Test entry point: apply one or more game-relative bundles with an
    /// explicit effective edge. The caller must not hold the game lock.
    #[cfg(test)]
    pub(crate) fn texture_apply(profile_edge: u32, level: u8, root: &Path, rels: &[PathBuf])
        -> io::Result<Vec<ApplyOutcome>> {
        if !(16..=16384).contains(&profile_edge) {
            return Err(invalid("Unity bundle apply requires an effective edge of 16..=16384"));
        }
        if !fs::symlink_metadata(root)?.file_type().is_dir() {
            return Err(invalid("Unity bundle apply requires a real game directory"));
        }
        let _guard = crate::assets::lock_game_for_mutation(root)?;
        let tree = crate::Tree::for_asset_writes(root)?;
        let mut outcomes = Vec::with_capacity(rels.len());
        for relative in rels {
            crate::cancelled()?;
            outcomes.push(apply_one(&tree, profile_edge, level, root, relative)?);
        }
        Ok(outcomes)
    }

    /// CLI wrapper: resolve the profile, hold the mutation lock once, run every
    /// bundle and print the machine-readable report.
    #[cfg(feature = "development-audits")]
    pub(crate) fn texture_apply_cli(target: &str, level: u8, root: &Path, rels: &[PathBuf]) -> io::Result<()> {
        let Some(edge) = crate::assets::profile_max_edge(target)? else {
            println!("UNITYFS_TEXTURE_APPLY|NATIVE|no-op profile; nothing was opened or written");
            return Ok(());
        };
        if edge == u32::MAX {
            println!("UNITYFS_TEXTURE_APPLY|LOSSLESS|the lossless profile never resizes textures; nothing to apply");
            return Ok(());
        }
        if !(16..=16384).contains(&edge) {
            return Err(invalid("Unity bundle apply requires a profile edge of 16..=16384"));
        }
        if !fs::symlink_metadata(root)?.file_type().is_dir() {
            return Err(invalid("Unity bundle apply requires a real game directory"));
        }
        eprintln!("Unity bundle apply integrity rule: a bundle is refused when its info-section uncompressed-data hash is nonzero, or when its path has an 'aa' component / a catalog.json or catalog.hash exists in a directory above it inside the game root (an Addressables catalogue may carry a hash this build cannot recompute). A catalogue stored elsewhere is not detectable here.");
        let _guard = crate::assets::lock_game_for_mutation(root)?;
        let tree = crate::Tree::for_asset_writes(root)?;
        let mut failures = 0usize;
        for relative in rels {
            match apply_one(&tree, edge, level, root, relative) {
                Ok(outcome) => print_outcome(&outcome),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(error) => {
                    eprintln!("bgc-native: Unity bundle apply failed for {}: {error}", relative.display());
                    failures += 1;
                }
            }
        }
        if failures != 0 {
            return Err(invalid(&format!("Unity bundle apply failed for {failures} bundle(s); see messages above")));
        }
        Ok(())
    }

    #[cfg(feature = "development-audits")]
    fn print_outcome(outcome: &ApplyOutcome) {
        let name = field(outcome.relative.as_os_str().as_bytes());
        for skip in &outcome.skips {
            println!("UNITYFS_TEXTURE_APPLY_SKIP|{name}|{}", field(skip.as_bytes()));
        }
        for texture in &outcome.textures {
            println!("UNITYFS_TEXTURE_APPLY_TEXTURE|{name}|{}|{}|{}x{}|{}x{}|{}|{}|{}|{}|{}|{}|{}|{}",
                texture.node_index, texture.path_id,
                texture.old_width, texture.old_height, texture.new_width, texture.new_height,
                texture.format, crate::unity_tree::format_name(texture.format),
                texture.old_mips, texture.new_mips, texture.old_len, texture.new_len,
                texture.storage, texture.mode);
        }
        match outcome.status {
            ApplyStatus::Applied => println!("UNITYFS_TEXTURE_APPLY|APPLIED|{name}|{}|{}|{}|{}|{}",
                outcome.applied, outcome.skipped, outcome.old_bytes, outcome.new_bytes, outcome.removed_decoded),
            ApplyStatus::NoGain => println!("UNITYFS_TEXTURE_APPLY|NO_GAIN|{name}|{}|{}", outcome.skipped, outcome.old_bytes),
            ApplyStatus::AlreadyOptimized => println!("UNITYFS_TEXTURE_APPLY|ALREADY_OPTIMIZED|{name}|{}", outcome.old_bytes),
            ApplyStatus::Refused => println!("UNITYFS_TEXTURE_APPLY|REFUSED|{name}|{}",
                field(outcome.refusal.as_deref().unwrap_or("unspecified refusal").as_bytes())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(version: u32, flags: u32, block_codec: u16) -> Vec<u8> {
        let mut seed = 0x12345678u32;
        let mut data = Vec::new();
        for _ in 0..8192 { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; data.push(seed as u8); }
        let pattern = data.clone();
        for _ in 0..15 { data.extend_from_slice(&pattern); }
        let encoded = if block_codec == 0 { data.clone() } else if block_codec == 2 {
            // Valid, deliberately poor LZ4: a literal-only block. Guarantees
            // coverage of the positive-gain path independently of LZ4 heuristics.
            let mut encoded = vec![0xf0]; let mut n = data.len() - 15;
            while n >= 255 { encoded.push(255); n -= 255; }
            encoded.push(n as u8); encoded.extend_from_slice(&data); encoded
        } else { compress(&data, Some(CompressionMode::FAST(1)), false).unwrap() };
        let mut info = vec![0; 16];
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&(data.len() as u32).to_be_bytes());
        info.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        info.extend_from_slice(&block_codec.to_be_bytes());
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(data.len() as u64).to_be_bytes());
        info.extend_from_slice(&0u32.to_be_bytes()); info.extend_from_slice(b"CAB-fixture.resS\0");
        let metadata = if flags & 0x3f == 0 { info.clone() } else { compress(&info, None, false).unwrap() };
        let mut b = b"UnityFS\0".to_vec(); b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(b"5.x.x\02021.3.45f1\0"); let fields = b.len();
        b.extend_from_slice(&0u64.to_be_bytes()); b.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes()); b.extend_from_slice(&flags.to_be_bytes());
        if version >= 7 { b.resize(align(b.len()).unwrap(), 0); }
        if flags & 0x80 == 0 { b.extend_from_slice(&metadata); }
        if flags & 0x200 != 0 { b.resize(align(b.len()).unwrap(), 0); }
        b.extend_from_slice(&encoded);
        if flags & 0x80 != 0 { b.extend_from_slice(&metadata); }
        let size = b.len() as u64; b[fields..fields + 8].copy_from_slice(&size.to_be_bytes()); b
    }
    #[test]
    fn roundtrips_all_supported_layouts_and_codecs() {
        for version in 6..=8 { for location in [0, 0x80, 0x200, 0x280] { for codec in [0, 2, 3] {
            let b = fixture(version, 0x40 | location | codec, codec as u16);
            let (out, _, old) = rebuild(&b).unwrap(); let new = parse(&out).unwrap();
            assert_eq!(old.flags, new.flags); assert_eq!(old.decoded, new.decoded);
            assert_eq!(old.info[30..], new.info[30..]); // Resource directory is identical.
            if codec == 2 { assert!(out.len() < b.len()); }
        } } }
    }
    #[test]
    fn rejects_truncation_unknown_flags_and_bad_extents() {
        let b = fixture(8, 0x243, 3);
        for n in [0, 7, 25, b.len() - 1] { assert!(rebuild(&b[..n]).is_err()); }
        let mut bad = b.clone(); let p = parse(&b).unwrap();
        bad[p.fields + 16..p.fields + 20].copy_from_slice(&0x343u32.to_be_bytes()); assert!(parse(&bad).is_err());
        let mut raw = fixture(6, 0x40, 0); let p = parse(&raw).unwrap();
        let start = p.header_end;
        raw[start + 34..start + 42].copy_from_slice(&u64::MAX.to_be_bytes()); assert!(parse(&raw).is_err());
    }
    #[test]
    fn stored_blocks_are_not_switched_to_another_codec() {
        let b = fixture(8, 0x240, 0); let (out, changed, _) = rebuild(&b).unwrap();
        assert_eq!(changed, 0); assert_eq!(b, out);
    }
    #[test]
    fn export_never_overwrites_and_low_efficiency_writes_nothing() {
        let root = std::env::temp_dir().join(format!("bgc-unityfs-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap(); let input = root.join("input"); let output = root.join("output");
        let b = fixture(8, 0x243, 3); fs::write(&input, &b).unwrap(); fs::write(&output, b"keep").unwrap();
        assert!(run(&input, Some(&output), 0.0).is_err()); assert_eq!(fs::read(&output).unwrap(), b"keep");
        assert!(run(&input, Some(&input), 0.0).is_err());
        let skipped = root.join("skipped"); run(&input, Some(&skipped), 100.0).unwrap(); assert!(!skipped.exists());
        assert_eq!(fs::read(&input).unwrap(), b); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bundle_inventory_is_read_only_and_lists_nodes() {
        let root = std::env::temp_dir().join(format!("bgc-unityfs-inv-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("bundle");
        let b = fixture(8, 0x242, 2);
        fs::write(&input, &b).unwrap();
        inventory(&input).unwrap();
        assert_eq!(fs::read(&input).unwrap(), b, "inventory must not modify the bundle");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn selected_node_decoder_checks_bounds_and_charges_work() {
        let bytes = fixture(8, 0x242, 2);
        let bundle = parse(&bytes).unwrap();
        let block = &bundle.blocks[0];
        let expected = decode(&bytes[block.offset..block.offset + block.encoded],
            block.decoded, (block.flags & 0x3f) as u32).unwrap();
        let mut decoder = NodeDecoder::new(&bytes, &bundle);
        assert_eq!(decoder.read(0, 16).unwrap(), expected[..16]);
        assert_eq!(decoder.read(bundle.decoded - 2, 2).unwrap(), expected[expected.len() - 2..]);
        assert!(decoder.read(bundle.decoded - 1, 2).is_err());
        decoder.copy_budget = 1;
        assert!(crate::audit_io::is_budget_error(&decoder.read(0, 2).unwrap_err()));
        let mut decoder = NodeDecoder::new(&bytes, &bundle);
        decoder.decode_budget = 0;
        assert!(crate::audit_io::is_budget_error(&decoder.read(0, 1).unwrap_err()));
    }
    #[test]
    fn known_unity_texture_formats_have_bounded_mip_shapes() {
        for (format, expected) in [(4, 320), (14, 320), (10, 40), (12, 80),
                                   (25, 80), (26, 40), (27, 80)] {
            let texture = crate::unity_tree::Texture { width: 8, height: 8,
                format, mips: 2, inline_bytes: 0, stream_offset: 0, stream_size: 0,
                stream_path: String::new() };
            assert_eq!(texture_bytes(&texture).unwrap(), Some(expected), "format {format}");
        }
        let unknown = crate::unity_tree::Texture { width: 8, height: 8,
            format: 28, mips: 2, inline_bytes: 0, stream_offset: 0, stream_size: 0,
            stream_path: String::new() };
        assert_eq!(texture_bytes(&unknown).unwrap(), None); // Crunch is not ordinary BC1 data.
    }
    fn put32(b: &mut Vec<u8>, v: u32) { b.extend_from_slice(&v.to_le_bytes()); }
    fn put64(b: &mut Vec<u8>, v: u64) { b.extend_from_slice(&v.to_le_bytes()); }
    /// A Texture2D type tree with explicit single-image/2D shape metadata.
    fn texture_tree() -> (Vec<u8>, Vec<u8>) {
        let specs = [
            (0, "Texture2D", "Base", -1, 0),
            (1, "int", "m_Width", 4, 0), (1, "int", "m_Height", 4, 0),
            (1, "int", "m_CompleteImageSize", 4, 0),
            (1, "int", "m_TextureFormat", 4, 0), (1, "int", "m_MipCount", 4, 0),
            (1, "int", "m_ImageCount", 4, 0), (1, "int", "m_TextureDimension", 4, 0),
            (1, "TypelessData", "image data", -1, 0x4000),
            (1, "StreamingInfo", "m_StreamData", -1, 0),
            (2, "UInt64", "offset", 8, 0), (2, "UInt32", "size", 4, 0),
            (2, "string", "path", -1, 0x4000),
        ];
        let mut strings = Vec::new(); let mut raw = Vec::new();
        for (level, kind, name, size, flags) in specs {
            let k = strings.len() as u32; strings.extend(kind.as_bytes()); strings.push(0);
            let n = strings.len() as u32; strings.extend(name.as_bytes()); strings.push(0);
            let mut node = vec![0; 32]; node[2] = level;
            for (off, value) in [(4, k), (8, n), (12, size as u32), (20, flags)] {
                node[off..off + 4].copy_from_slice(&value.to_le_bytes());
            }
            raw.extend(node);
        }
        (raw, strings)
    }
    fn inline_object(width: u32, height: u32, format: u32, mips: u32, payload: &[u8]) -> Vec<u8> {
        let mut o = Vec::new();
        for v in [width, height, payload.len() as u32, format, mips, 1, 2] { o.extend_from_slice(&v.to_le_bytes()); }
        o.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        o.extend_from_slice(payload);
        o.extend_from_slice(&0u64.to_le_bytes()); // stream offset
        o.extend_from_slice(&0u32.to_le_bytes()); // stream size (inline only)
        o.extend_from_slice(&0u32.to_le_bytes()); // stream path length
        while o.len() % 4 != 0 { o.push(0); }
        o
    }
    /// One v22 little-endian SerializedFile holding the given texture objects
    /// (path ID, object bytes) at 16-byte-aligned data offsets.
    fn serialized_objects(objects: &[(u64, Vec<u8>)]) -> Vec<u8> {
        let (raw, strings) = texture_tree();
        let mut m = b"2022.3.0f1\0".to_vec();
        put32(&mut m, 19); m.push(1); put32(&mut m, 1);
        put32(&mut m, 28); m.push(0); m.extend([255; 2]); m.extend([0; 16]);
        put32(&mut m, (raw.len() / 32) as u32); put32(&mut m, strings.len() as u32);
        m.extend(&raw); m.extend(&strings);
        put32(&mut m, 0); // dependencies
        put32(&mut m, objects.len() as u32); while m.len() % 4 != 0 { m.push(0); }
        let mut payload = Vec::new();
        for (index, (id, object)) in objects.iter().enumerate() {
            if index > 0 { while payload.len() % 16 != 0 { payload.push(0); } }
            put64(&mut m, *id); put64(&mut m, payload.len() as u64);
            put32(&mut m, object.len() as u32); put32(&mut m, 0);
            payload.extend_from_slice(object);
        }
        for _ in 0..3 { put32(&mut m, 0); } m.push(0);
        let data = (48 + m.len() as u64).div_ceil(16) * 16;
        let mut sf = vec![0u8; 48];
        sf[8..12].copy_from_slice(&22u32.to_be_bytes());
        sf[20..24].copy_from_slice(&(m.len() as u32).to_be_bytes());
        sf[24..32].copy_from_slice(&(data + payload.len() as u64).to_be_bytes());
        sf[32..40].copy_from_slice(&data.to_be_bytes());
        sf.extend(&m); sf.resize(data as usize, 0); sf.extend(&payload);
        sf
    }
    /// One v22 little-endian SerializedFile with a single Texture2D object (id 123).
    fn serialized_inline_texture(object: &[u8]) -> Vec<u8> {
        serialized_objects(&[(123, object.to_vec())])
    }
    /// A stored UnityFS bundle whose single node is a flag-4 SerializedFile.
    fn bundle_with_serialized(data: &[u8]) -> Vec<u8> {
        let mut info = vec![0; 16];
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&(data.len() as u32).to_be_bytes());
        info.extend_from_slice(&(data.len() as u32).to_be_bytes());
        info.extend_from_slice(&0u16.to_be_bytes()); // stored block
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(data.len() as u64).to_be_bytes());
        info.extend_from_slice(&4u32.to_be_bytes()); // SerializedFile directory flag
        info.extend_from_slice(b"CAB-test\0");
        let mut b = b"UnityFS\0".to_vec();
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\02021.3.45f1\0"); let fields = b.len();
        b.extend_from_slice(&0u64.to_be_bytes()); b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes()); b.extend_from_slice(&0x40u32.to_be_bytes());
        b.resize(align(b.len()).unwrap(), 0);
        b.extend_from_slice(&info);
        b.extend_from_slice(data);
        let size = b.len() as u64; b[fields..fields + 8].copy_from_slice(&size.to_be_bytes()); b
    }
    /// One v22 SerializedFile plus a same-bundle resource node holding `stream`.
    fn bundle_with_serialized_and_stream(sf: &[u8], stream: &[u8]) -> Vec<u8> {
        let mut data = sf.to_vec();
        data.extend_from_slice(stream);
        let mut info = vec![0; 16];
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&(data.len() as u32).to_be_bytes());
        info.extend_from_slice(&(data.len() as u32).to_be_bytes());
        info.extend_from_slice(&0u16.to_be_bytes());
        info.extend_from_slice(&2u32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(sf.len() as u64).to_be_bytes());
        info.extend_from_slice(&4u32.to_be_bytes());
        info.extend_from_slice(b"CAB-test\0");
        info.extend_from_slice(&(sf.len() as u64).to_be_bytes());
        info.extend_from_slice(&(stream.len() as u64).to_be_bytes());
        info.extend_from_slice(&0u32.to_be_bytes());
        info.extend_from_slice(b"CAB-test.resS\0");
        let mut b = b"UnityFS\0".to_vec();
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\02021.3.45f1\0"); let fields = b.len();
        b.extend_from_slice(&0u64.to_be_bytes()); b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes()); b.extend_from_slice(&0x40u32.to_be_bytes());
        b.resize(align(b.len()).unwrap(), 0);
        b.extend_from_slice(&info);
        b.extend_from_slice(&data);
        let size = b.len() as u64; b[fields..fields + 8].copy_from_slice(&size.to_be_bytes()); b
    }
    fn streamed_object(path: &str, size: u32) -> Vec<u8> {
        let mut o = Vec::new();
        for v in [8u32, 8, size, 4, 1, 1, 2] { o.extend_from_slice(&v.to_le_bytes()); }
        o.extend_from_slice(&0u32.to_le_bytes()); // inline length 0
        o.extend_from_slice(&0u64.to_le_bytes()); // stream offset 0
        o.extend_from_slice(&size.to_le_bytes()); // stream size
        o.extend_from_slice(&(path.len() as u32).to_le_bytes());
        o.extend_from_slice(path.as_bytes());
        while o.len() % 4 != 0 { o.push(0); }
        o
    }
    /// One streamed texture object with an explicit declared shape and stream
    /// offset. The caller supplies the declared extent so shape/length
    /// disagreement can be tested as well as the accepted shrink shapes.
    fn streamed_object_at(width: u32, height: u32, format: u32, offset: u64, size: u32, path: &str) -> Vec<u8> {
        let mut o = Vec::new();
        for v in [width, height, size, format, full_mip_levels(width, height), 1, 2] {
            o.extend_from_slice(&v.to_le_bytes());
        }
        o.extend_from_slice(&0u32.to_le_bytes()); // inline length 0
        o.extend_from_slice(&offset.to_le_bytes()); // stream offset
        o.extend_from_slice(&size.to_le_bytes()); // stream size
        o.extend_from_slice(&(path.len() as u32).to_le_bytes());
        o.extend_from_slice(path.as_bytes());
        while o.len() % 4 != 0 { o.push(0); }
        o
    }
    /// The same helper with a zero offset and the complete stream extent of the
    /// declared shape, as a real streamed texture would declare it.
    fn streamed_object_shape(width: u32, height: u32, format: u32, path: &str) -> Vec<u8> {
        streamed_object_at(width, height, format, 0, chain_len(format, width, height) as u32, path)
    }
    #[test]
    fn inline_texture_exports_verbatim_payload_to_verified_dds() {
        let payload: Vec<u8> = (0..256u32).map(|i| (i * 7 % 251) as u8).collect();
        let object = inline_object(8, 8, 4, 1, &payload);
        let bundle = bundle_with_serialized(&serialized_inline_texture(&object));
        let root = std::env::temp_dir().join(format!("bgc-unitytex-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let output = root.join("texture.dds");
        texture_export(0, 123, &input, &output).unwrap();
        // The source bundle is never rewritten, and the artifact is a valid DDS
        // whose payload is the verbatim inline bytes.
        assert_eq!(fs::read(&input).unwrap(), bundle);
        let dds = fs::read(&output).unwrap();
        let parsed = crate::texture::parse(&dds).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.mipmaps), (8, 8, 1));
        assert_eq!(&dds[parsed.data_start..], &payload[..]);
        // Never overwrites an existing destination, and refuses unknown identity
        // or a non-SerializedFile node instead of guessing.
        assert!(texture_export(0, 123, &input, &output).is_err());
        assert!(texture_export(0, 999, &input, &root.join("other.dds")).is_err());
        assert!(texture_export(1, 123, &input, &root.join("other2.dds")).is_err());
        // A streamed variant (no inline payload, non-empty stream extent) is refused.
        let mut streamed = Vec::new();
        for v in [8u32, 8, 4, 4, 1, 1, 2] { streamed.extend_from_slice(&v.to_le_bytes()); }
        streamed.extend_from_slice(&0u32.to_le_bytes()); // inline length 0
        streamed.extend_from_slice(&0u64.to_le_bytes()); // stream offset
        streamed.extend_from_slice(&4u32.to_le_bytes()); // stream size
        streamed.extend_from_slice(&3u32.to_le_bytes()); // path length
        streamed.extend_from_slice(b"a.b");
        while streamed.len() % 4 != 0 { streamed.push(0); }
        let streamed_bundle = bundle_with_serialized(&serialized_inline_texture(&streamed));
        let streamed_input = root.join("streamed"); fs::write(&streamed_input, &streamed_bundle).unwrap();
        assert!(texture_export(0, 123, &streamed_input, &root.join("streamed.dds")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn streamed_extent_exports_when_uniquely_claimed() {
        let payload: Vec<u8> = (0..256u32).map(|i| (i * 5 % 251) as u8).collect();
        let object = streamed_object("CAB-test.resS", payload.len() as u32);
        let sf = serialized_inline_texture(&object);
        let bundle = bundle_with_serialized_and_stream(&sf, &payload);
        let root = std::env::temp_dir().join(format!("bgc-unitystream-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let output = root.join("streamed.dds");
        texture_export(0, 123, &input, &output).unwrap();
        // The source bundle is never rewritten and the artifact holds the exact
        // declared stream extent.
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let dds = fs::read(&output).unwrap();
        let parsed = crate::texture::parse(&dds).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.mipmaps), (8, 8, 1));
        assert_eq!(&dds[parsed.data_start..], &payload[..]);
        // A stream path with no matching same-bundle node is refused, not guessed.
        let missing = streamed_object("missing.resS", 256);
        let missing_bundle = bundle_with_serialized(&serialized_inline_texture(&missing));
        let missing_input = root.join("missing"); fs::write(&missing_input, &missing_bundle).unwrap();
        assert!(texture_export(0, 123, &missing_input, &root.join("missing.dds")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn beneficial_export_preserves_source_and_refuses_symlinks() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("bgc-unityfs-export-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap(); let input = root.join("input"); let output = root.join("output");
        let b = fixture(8, 0x242, 2); fs::write(&input, &b).unwrap();
        run(&input, Some(&output), 5.0).unwrap();
        let exported = fs::read(&output).unwrap(); assert!(exported.len() < b.len());
        assert_eq!(parse(&exported).unwrap().decoded, parse(&b).unwrap().decoded);
        assert_eq!(fs::read(&input).unwrap(), b);
        let link = root.join("source-link"); symlink(&input, &link).unwrap(); assert!(run(&link, None, 5.0).is_err());
        let dangling = root.join("dangling"); symlink(root.join("absent"), &dangling).unwrap();
        assert!(run(&input, Some(&dangling), 5.0).is_err());
        assert!(run(&input, None, f64::NAN).is_err()); assert!(run(&input, None, -1.0).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    /// A UnityFS bundle with an explicit node table, arbitrary block splits and a
    /// caller-chosen uncompressed-data hash field. `nodes` are (offset, size,
    /// flags, path) into the concatenated node bytes; `block_sizes` must sum to
    /// their total.
    fn bundle_with_blocks(data: &[u8], nodes: &[(u64, u64, u32, &str)], block_sizes: &[usize],
        codec: u16, hash: [u8; 16]) -> Vec<u8> {
        bundle_with_encoder(data, nodes, block_sizes, codec, hash, |chunk| {
            if codec == 0 { chunk.to_vec() }
            else { compress(chunk, Some(CompressionMode::HIGHCOMPRESSION(9)), false).unwrap() }
        })
    }
    /// The same bundle builder with an explicit block/metadata encoder.
    fn bundle_with_encoder(data: &[u8], nodes: &[(u64, u64, u32, &str)], block_sizes: &[usize],
        codec: u16, hash: [u8; 16], encode: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
        assert_eq!(block_sizes.iter().sum::<usize>(), data.len());
        let mut info = hash.to_vec();
        info.extend_from_slice(&(block_sizes.len() as u32).to_be_bytes());
        let mut blocks = Vec::new();
        let mut offset = 0usize;
        for size in block_sizes {
            let encoded = encode(&data[offset..offset + size]);
            assert_eq!(decode(&encoded, *size, codec as u32).unwrap(), &data[offset..offset + size]);
            info.extend_from_slice(&(*size as u32).to_be_bytes());
            info.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
            info.extend_from_slice(&codec.to_be_bytes());
            blocks.push(encoded);
            offset += size;
        }
        info.extend_from_slice(&(nodes.len() as u32).to_be_bytes());
        for (offset, size, flags, path) in nodes {
            info.extend_from_slice(&offset.to_be_bytes());
            info.extend_from_slice(&size.to_be_bytes());
            info.extend_from_slice(&flags.to_be_bytes());
            info.extend_from_slice(path.as_bytes()); info.push(0);
        }
        let metadata = if codec == 0 { info.clone() } else { encode(&info) };
        let mut b = b"UnityFS\0".to_vec();
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\02021.3.45f1\0"); let fields = b.len();
        b.extend_from_slice(&0u64.to_be_bytes()); b.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes()); b.extend_from_slice(&(0x40u32 | codec as u32).to_be_bytes());
        b.resize(align(b.len()).unwrap(), 0);
        b.extend_from_slice(&metadata);
        for encoded in blocks { b.extend_from_slice(&encoded); }
        let length = b.len() as u64; b[fields..fields + 8].copy_from_slice(&length.to_be_bytes()); b
    }
    /// A valid, deliberately literal-only LZ4 block. Decodable but always beaten
    /// by the high-compression re-encode, so a patched bundle is provably
    /// smaller without depending on LZ4 heuristics.
    fn literal_lz4(data: &[u8]) -> Vec<u8> {
        let mut encoded = vec![(data.len().min(15) as u8) << 4];
        let mut n = data.len().saturating_sub(15);
        while n >= 255 { encoded.push(255); n -= 255; }
        if data.len() >= 15 { encoded.push(n as u8); }
        encoded.extend_from_slice(data); encoded
    }
    /// A bundle whose blocks and metadata use the poor LZ4 encoding above, with
    /// a single SerializedFile node.
    fn bundle_with_poor_blocks(sf: &[u8]) -> Vec<u8> {
        bundle_with_encoder(sf, &[(0, sf.len() as u64, 4, "CAB-test")], &[sf.len()], 3, [0u8; 16], literal_lz4)
    }
    /// Object-relative payload offset of the single inline Texture2D in `sf`.
    fn inline_payload_offset(sf: &[u8], path_id: u64) -> usize {
        let mut metadata_budget = 32u64 * 1024 * 1024;
        let mut object_budget = 64u64 * 1024 * 1024;
        let mut report_budget = 10000u64;
        let inventory = crate::unity_serialized::bundle_texture_inventory(
            sf, &mut metadata_budget, &mut object_budget, &mut report_budget).unwrap();
        let (_, fields) = inventory.details.iter().find(|(id, _)| *id == path_id).unwrap();
        let record = inventory.object_records.iter().find(|record| record.id == path_id).unwrap();
        record.start as usize + fields.spans.get("image data").unwrap().payload.unwrap().0
    }
    #[test]
    fn inline_texture_replace_publishes_a_verified_bundle_and_keeps_the_source() {
        let payload: Vec<u8> = (0..256u32).map(|i| (i * 7 % 251) as u8).collect();
        let object = inline_object(8, 8, 4, 1, &payload);
        let sf = serialized_inline_texture(&object);
        // A poor (literal-only) source encoding guarantees the re-encoded block
        // is strictly smaller, so the same-shape replace is published.
        let bundle = bundle_with_poor_blocks(&sf);
        let replacement: Vec<u8> = (0..256u32).map(|i| ((i * 11 + 3) % 253) as u8).collect();
        let root = std::env::temp_dir().join(format!("bgc-unitytex-replace-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &replacement).unwrap();
        let output = root.join("replaced.bundle");
        texture_replace(0, 123, &payload_file, &input, &output).unwrap();
        // The source is never modified, and the candidate parses with the same
        // layout and the same preserved zero hash placeholder.
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let out = fs::read(&output).unwrap();
        let (parsed, original) = (parse(&out).unwrap(), parse(&bundle).unwrap());
        assert_eq!(parsed.nodes.len(), original.nodes.len());
        assert_eq!(parsed.decoded, original.decoded);
        assert_eq!(parsed.flags, original.flags);
        assert_eq!(parsed.info[..16], original.info[..16], "placeholder hash field is preserved");
        // The replaced payload re-extracts through the existing export route.
        let dds_path = root.join("check.dds");
        texture_export(0, 123, &output, &dds_path).unwrap();
        let dds = fs::read(&dds_path).unwrap();
        let check = crate::texture::parse(&dds).unwrap();
        assert_eq!((check.width, check.height, check.mipmaps), (8, 8, 1));
        assert_eq!(&dds[check.data_start..], &replacement[..]);
        // A destination that already exists is refused, not replaced.
        assert!(texture_replace(0, 123, &payload_file, &input, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), out);
        // Unknown/missing identities and an out-of-range node are refused.
        assert!(texture_replace(0, 999, &payload_file, &input, &root.join("wrong.bundle")).is_err());
        assert!(texture_replace(7, 123, &payload_file, &input, &root.join("oob.bundle")).is_err());
        // Payloads that are not the known mip-chain length are refused unwritten.
        let short = root.join("short.bin"); fs::write(&short, &replacement[..255]).unwrap();
        let long = root.join("long.bin");
        let mut extra = replacement.clone(); extra.push(0); fs::write(&long, &extra).unwrap();
        for (name, path) in [("short.out", &short), ("long.out", &long)] {
            let destination = root.join(name);
            assert!(texture_replace(0, 123, path, &input, &destination).is_err(), "{name}");
            assert!(!destination.exists());
        }
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn compressed_multi_block_replace_only_rewrites_the_touched_blocks() {
        let payload: Vec<u8> = (0..256u32).map(|i| (i * 7 % 251) as u8).collect();
        let object = inline_object(8, 8, 4, 1, &payload);
        let sf = serialized_inline_texture(&object);
        let stream: Vec<u8> = (0..8192u32).map(|i| (i * 13 % 249) as u8).collect();
        let mut data = sf.clone(); data.extend_from_slice(&stream);
        // Block boundaries fall inside the payload so the patch spans blocks, and
        // two later blocks cover the object trailer and the untouched .resS node.
        let payload_at = inline_payload_offset(&sf, 123);
        let payload_end = payload_at + payload.len();
        let mut cuts = vec![payload_at + 3, payload_at + 9, payload_end, sf.len(), data.len()];
        cuts.sort_unstable(); cuts.dedup();
        let mut sizes = Vec::new();
        let mut previous = 0usize;
        for cut in cuts { sizes.push(cut - previous); previous = cut; }
        let nodes = [(0u64, sf.len() as u64, 4u32, "CAB-test"),
                     (sf.len() as u64, stream.len() as u64, 0u32, "CAB-test.resS")];
        // Poor source encoding: the touched blocks are re-encoded smaller, so the
        // strictly-smaller publication gate passes.
        let bundle = bundle_with_encoder(&data, &nodes, &sizes, 3, [0u8; 16], literal_lz4);
        let replacement: Vec<u8> = (0..256u32).map(|i| ((i * 5 + 9) % 251) as u8).collect();
        let root = std::env::temp_dir().join(format!("bgc-unitytex-blocks-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &replacement).unwrap();
        let output = root.join("replaced.bundle");
        texture_replace(0, 123, &payload_file, &input, &output).unwrap();
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let out = fs::read(&output).unwrap();
        let (old, new) = (parse(&bundle).unwrap(), parse(&out).unwrap());
        assert_eq!(old.blocks.len(), new.blocks.len());
        // Untouched blocks keep byte-identical compressed payloads.
        let mut decoded_start = 0usize;
        for (old_block, new_block) in old.blocks.iter().zip(&new.blocks) {
            let decoded_end = decoded_start + old_block.decoded;
            let touched = decoded_start < payload_end && payload_at < decoded_end;
            if !touched {
                assert_eq!(&bundle[old_block.offset..old_block.offset + old_block.encoded],
                    &out[new_block.offset..new_block.offset + new_block.encoded],
                    "untouched block at {decoded_start} must be byte-identical");
            }
            decoded_start = decoded_end;
        }
        // The .resS node is unchanged and the replaced payload reads back exactly.
        let mut decoder = NodeDecoder::new(&out, &new);
        assert_eq!(decoder.read(new.nodes[1].offset, new.nodes[1].size).unwrap(), stream);
        let dds_path = root.join("check.dds");
        texture_export(0, 123, &output, &dds_path).unwrap();
        let dds = fs::read(&dds_path).unwrap();
        let check = crate::texture::parse(&dds).unwrap();
        assert_eq!(&dds[check.data_start..], &replacement[..]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn replace_refuses_unprovable_payloads_hashes_and_layouts() {
        let good: Vec<u8> = (0..256u32).map(|i| (i * 7 % 251) as u8).collect();
        let root = std::env::temp_dir().join(format!("bgc-unitytex-refuse-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &good).unwrap();
        // (a) An inline length that disagrees with the known mip chain is refused.
        let short_object = inline_object(8, 8, 4, 1, &good[..250]);
        let short_bundle = bundle_with_serialized(&serialized_inline_texture(&short_object));
        let short_input = root.join("short"); fs::write(&short_input, &short_bundle).unwrap();
        let error = texture_replace(0, 123, &payload_file, &short_input, &root.join("short.out")).unwrap_err();
        assert!(format!("{error}").contains("disagrees"), "{error}");
        assert!(!root.join("short.out").exists());
        // (b) A nonzero uncompressed-data hash cannot be recomputed, so the source
        // is refused rather than published with a stale field.
        let object = inline_object(8, 8, 4, 1, &good);
        let sf = serialized_inline_texture(&object);
        let hashed = bundle_with_blocks(&sf, &[(0, sf.len() as u64, 4, "CAB-test")],
            &[sf.len()], 0, [0x5a; 16]);
        let hashed_input = root.join("hashed"); fs::write(&hashed_input, &hashed).unwrap();
        let error = texture_replace(0, 123, &payload_file, &hashed_input, &root.join("hashed.out")).unwrap_err();
        assert!(format!("{error}").contains("uncompressed-data hash"), "{error}");
        assert!(!root.join("hashed.out").exists());
        // (c) A streamed texture now takes the two-node streamed route; an
        // equal-length payload is refused there because that route is shrink-only
        // (the zero-padded old extent is the point).
        let stream: Vec<u8> = (0..256u32).map(|i| (i * 3 % 251) as u8).collect();
        let streamed_sf = serialized_inline_texture(&streamed_object("CAB-test.resS", 256));
        let streamed = bundle_with_serialized_and_stream(&streamed_sf, &stream);
        let streamed_input = root.join("streamed"); fs::write(&streamed_input, &streamed).unwrap();
        let error = texture_replace(0, 123, &payload_file, &streamed_input, &root.join("streamed.out")).unwrap_err();
        assert!(format!("{error}").contains("strictly shorter"), "{error}");
        assert!(!root.join("streamed.out").exists());
        // (d) A non-bundle input fails closed, and the source files stay untouched.
        let garbage = root.join("garbage"); fs::write(&garbage, b"not a UnityFS bundle").unwrap();
        assert!(texture_replace(0, 123, &payload_file, &garbage, &root.join("garbage.out")).is_err());
        assert_eq!(fs::read(&short_input).unwrap(), short_bundle);
        assert_eq!(fs::read(&hashed_input).unwrap(), hashed);
        assert_eq!(fs::read(&streamed_input).unwrap(), streamed);
        fs::remove_dir_all(root).unwrap();
    }
    /// Chain length of the complete 1x1-terminated chain of one shape.
    fn chain_len(format: u32, width: u32, height: u32) -> usize {
        texture_shape_bytes(format, width, height, full_mip_levels(width, height)).unwrap() as usize
    }
    fn payload_of(len: usize, seed: u32) -> Vec<u8> {
        (0..len as u32).map(|i| (i.wrapping_mul(seed).wrapping_add(7) % 251) as u8).collect()
    }
    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn shrunk_inline_texture_keeps_the_object_extent_and_publishes_a_smaller_bundle() {
        // 128x128 DXT1 with its complete 8-level chain. The 64x64 downscale is a
        // 7-level chain, passes the repository short-edge policy, and is spliced
        // into the same object extent with a zero-padded tail.
        let old_len = chain_len(10, 128, 128);
        assert_eq!(old_len, 10936);
        let new_len = chain_len(10, 64, 64);
        assert_eq!(new_len, 2744);
        let old_payload = payload_of(old_len, 7);
        let new_payload = payload_of(new_len, 11);
        let other = payload_of(8 * 8 * 4, 3);
        let objects = [(123u64, inline_object(128, 128, 10, 8, &old_payload)),
                       (124u64, inline_object(8, 8, 4, 1, &other))];
        let sf = serialized_objects(&objects);
        let bundle = bundle_with_poor_blocks(&sf);
        let root = scratch("bgc-unitytex-shrink");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &new_payload).unwrap();
        let output = root.join("shrunk.bundle");
        texture_replace(0, 123, &payload_file, &input, &output).unwrap();
        // (e) The source is never modified.
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let out = fs::read(&output).unwrap();
        assert!(out.len() < bundle.len(), "published bundle must be strictly smaller");
        // The container keeps its node/block/decoded layout and the SerializedFile
        // object table is byte-stable.
        let (before, after) = (parse(&bundle).unwrap(), parse(&out).unwrap());
        assert_eq!(before.nodes.len(), after.nodes.len());
        assert_eq!(before.blocks.len(), after.blocks.len());
        assert_eq!(before.decoded, after.decoded);
        let (mut old_decoder, mut new_decoder) = (NodeDecoder::new(&bundle, &before), NodeDecoder::new(&out, &after));
        let old_node = old_decoder.read(0, before.nodes[0].size).unwrap();
        let new_node = new_decoder.read(0, after.nodes[0].size).unwrap();
        let old_records = node_texture_inventory(&old_node).unwrap();
        let new_records = node_texture_inventory(&new_node).unwrap();
        assert_eq!(old_records.object_records.len(), new_records.object_records.len());
        for (old, new) in old_records.object_records.iter().zip(&new_records.object_records) {
            assert_eq!((old.id, old.start, old.size, old.type_index, old.table_start, old.table_end),
                (new.id, new.start, new.size, new.type_index, new.table_start, new.table_end));
        }
        // (d) Everything outside the target object extent is byte-identical: no
        // other object, table entry or node byte can have moved.
        let target = old_records.object_records.iter().find(|record| record.id == 123).unwrap();
        let (start, end) = (target.start as usize, (target.start + target.size) as usize);
        assert_eq!(&old_node[..start], &new_node[..start]);
        assert_eq!(&old_node[end..], &new_node[end..]);
        let before_other = old_records.object_records.iter().find(|record| record.id == 124).unwrap();
        let after_other = new_records.object_records.iter().find(|record| record.id == 124).unwrap();
        assert_eq!(&old_node[before_other.start as usize..(before_other.start + before_other.size) as usize],
            &new_node[after_other.start as usize..(after_other.start + after_other.size) as usize]);
        // (a) The patched object re-parses with the new declared shape and the
        // fields sit at the captured spans; the tail of the old payload extent is
        // zero, which is what keeps every later field's parsed value identical.
        let (_, fields) = new_records.details.iter().find(|(id, _)| *id == 123).unwrap();
        assert_eq!((fields.texture.width, fields.texture.height, fields.texture.mips), (64, 64, 7));
        assert_eq!(fields.texture.inline_bytes, new_len);
        assert_eq!((fields.image_count, fields.dimension), (Some(1), Some(2)));
        let object = &new_node[start..end];
        for (name, value) in [("m_Width", 64u32), ("m_Height", 64),
                              ("m_CompleteImageSize", new_len as u32), ("m_MipCount", 7)] {
            let span = fields.spans.get(name).unwrap();
            assert_eq!(read_u32(object, span.start, false).unwrap(), value, "{name}");
        }
        let span = fields.spans.get("image data").unwrap();
        let (offset, length) = span.payload.unwrap();
        assert_eq!(length, new_len);
        assert_eq!(span.start + 4, offset);
        assert_eq!(read_u32(object, span.start, false).unwrap(), new_len as u32);
        assert!(object[offset + length..].iter().all(|byte| *byte == 0), "old payload extent must be zero-padded");
        // The payload re-extracts byte-identically through the shared selection.
        let dds_path = root.join("check.dds");
        texture_export(0, 123, &output, &dds_path).unwrap();
        let dds = fs::read(&dds_path).unwrap();
        let check = crate::texture::parse(&dds).unwrap();
        assert_eq!((check.width, check.height, check.mipmaps), (64, 64, 7));
        assert_eq!(&dds[check.data_start..], &new_payload[..]);
        // A shrink that is not strictly smaller is not published, and an existing
        // destination is never replaced.
        assert!(texture_replace(0, 123, &payload_file, &input, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), out);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn shrink_refuses_upscale_policy_and_unmatched_chain_lengths() {
        let old_len = chain_len(10, 128, 128);
        let old_payload = payload_of(old_len, 7);
        let object = inline_object(128, 128, 10, 8, &old_payload);
        let sf = serialized_inline_texture(&object);
        let bundle = bundle_with_poor_blocks(&sf);
        let root = scratch("bgc-unitytex-shrink-refuse");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin");
        let cases = [
            // (b) An upscaled shape cannot be spliced: its chain is larger than the
            // current extent.
            ("upscale", payload_of(chain_len(10, 256, 256), 11), "larger than the current"),
            // (c) A 32x32 chain violates the repository short-edge policy.
            ("thin", payload_of(chain_len(10, 32, 32), 11), "texture policy"),
            // (c) One level short of the old chain matches no accepted shape.
            ("levels", payload_of(old_len - 8, 11), "must be exactly"),
            // (c) An arbitrary length matches nothing at all.
            ("odd", payload_of(old_len / 2 + 1, 11), "must be exactly"),
        ];
        for (name, payload, message) in cases {
            fs::write(&payload_file, &payload).unwrap();
            let destination = root.join(&format!("{name}.bundle"));
            let error = texture_replace(0, 123, &payload_file, &input, &destination).unwrap_err();
            assert!(format!("{error}").contains(message), "{name}: {error}");
            assert!(!destination.exists(), "{name} must not publish");
        }
        // A shrink needs the m_CompleteImageSize span (its value changes) and it
        // must hold exactly the old inline length.
        let shrink_file = root.join("shrink.bin");
        fs::write(&shrink_file, payload_of(chain_len(10, 64, 64), 11)).unwrap();
        let mut inconsistent = inline_object(128, 128, 10, 8, &old_payload);
        inconsistent[8..12].copy_from_slice(&0u32.to_le_bytes());
        let inconsistent_bundle = bundle_with_poor_blocks(&serialized_inline_texture(&inconsistent));
        let inconsistent_input = root.join("inconsistent");
        fs::write(&inconsistent_input, &inconsistent_bundle).unwrap();
        let error = texture_replace(0, 123, &shrink_file, &inconsistent_input,
            &root.join("inconsistent.bundle")).unwrap_err();
        assert!(format!("{error}").contains("does not hold the parsed value"), "{error}");
        // A source with nonzero bytes after the payload cannot be shrunk: the
        // fields that follow the payload would then be read from the zero tail.
        let mut stale = Vec::new();
        for v in [128u32, 128, old_len as u32, 10, 8, 1, 2] { stale.extend_from_slice(&v.to_le_bytes()); }
        stale.extend_from_slice(&(old_len as u32).to_le_bytes());
        stale.extend_from_slice(&old_payload);
        stale.extend_from_slice(&0xdead_beefu64.to_le_bytes()); // stale stream offset
        stale.extend_from_slice(&0u32.to_le_bytes());
        stale.extend_from_slice(&0u32.to_le_bytes());
        while stale.len() % 4 != 0 { stale.push(0); }
        let stale_bundle = bundle_with_poor_blocks(&serialized_inline_texture(&stale));
        let stale_input = root.join("stale"); fs::write(&stale_input, &stale_bundle).unwrap();
        let error = texture_replace(0, 123, &shrink_file, &stale_input,
            &root.join("stale.bundle")).unwrap_err();
        assert!(format!("{error}").contains("every byte after"), "{error}");
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn no_gain_replacements_publish_nothing() {
        // A stored-codec source keeps the object extent byte-for-byte, so both a
        // shrink and a same-shape payload rebuild to exactly the original size:
        // the route reports NO_GAIN and leaves the destination absent.
        let old_len = chain_len(10, 128, 128);
        let object = inline_object(128, 128, 10, 8, &payload_of(old_len, 7));
        let sf = serialized_inline_texture(&object);
        let bundle = bundle_with_serialized(&sf);
        let root = scratch("bgc-unitytex-nogain");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let shrink = root.join("shrink.bin");
        fs::write(&shrink, payload_of(chain_len(10, 64, 64), 11)).unwrap();
        let same = root.join("same.bin");
        fs::write(&same, payload_of(old_len, 13)).unwrap();
        for (name, payload) in [("shrink.bundle", &shrink), ("same.bundle", &same)] {
            let destination = root.join(name);
            texture_replace(0, 123, payload, &input, &destination).unwrap();
            assert!(!destination.exists(), "{name} must not publish a non-smaller rebuild");
        }
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rebuild_patches_applies_disjoint_ranges_and_refuses_overlaps() {
        let b = fixture(8, 0x243, 3);
        // Half-open ranges that merely touch are disjoint; a real overlap is
        // refused before any block is touched.
        assert!(rebuild_patches(&b, &[(0, &[9]), (8, &[7])]).is_ok());
        assert!(rebuild_patches(&b, &[(0, &[1, 2]), (1, &[3])]).is_err());
        let bundle = parse(&b).unwrap();
        let mut source_decoder = NodeDecoder::new(&b, &bundle);
        let first = source_decoder.read(0, bundle.blocks[0].decoded as u64).unwrap();
        let (out, _, _) = rebuild_patches(&b, &[(0, &[9]), (4, &[7])]).unwrap();
        let check = parse(&out).unwrap();
        let mut decoder = NodeDecoder::new(&out, &check);
        let mut expected = first;
        expected[0] = 9; expected[4] = 7;
        assert_eq!(decoder.read(0, expected.len() as u64).unwrap(), expected);
        assert!(rebuild_patches(&b, &[(bundle.decoded as usize, &[1])]).is_err());
    }
    #[test]
    fn streamed_shrink_physically_compacts_the_node_and_shifts_a_later_offset() {
        // 128x128 DXT1 (8 mips, 10936 stream bytes) downscaled to the 64x64 7-mip
        // chain (2744 bytes), so 8192 stream bytes are physically removed. The
        // target extent starts 64 bytes into the .resS node, another streamed
        // texture owns that kept prefix, and a third texture starts exactly at
        // the old extent end: compaction must shift that later m_StreamData/offset
        // down by the removed delta, shrink the .resS node and move the following
        // node down by the same delta. The extent is split across blocks that
        // straddle the cut on both sides, so piece handling is exercised too.
        let old_len = chain_len(10, 128, 128);
        assert_eq!(old_len, 10936);
        let new_len = chain_len(10, 64, 64);
        assert_eq!(new_len, 2744);
        let removed = (old_len - new_len) as u64;
        let prefix = 64u64;
        let before = payload_of(prefix as usize, 21);
        let target_payload = payload_of(old_len, 7);
        let after = payload_of(600, 29);
        let other = payload_of(300, 33);
        let target = streamed_object_at(128, 128, 10, prefix, old_len as u32, "CAB-test.resS");
        let before_object = streamed_object_at(8, 8, 10, 0, before.len() as u32, "CAB-test.resS");
        let after_object = streamed_object_at(8, 8, 10, prefix + old_len as u64, after.len() as u32, "CAB-test.resS");
        let sf = serialized_objects(&[(123, target), (124, before_object), (125, after_object)]);
        let mut data = sf.clone();
        data.extend_from_slice(&before);
        data.extend_from_slice(&target_payload);
        data.extend_from_slice(&after);
        data.extend_from_slice(&other);
        let stream_start = sf.len();
        let cut_from = stream_start + prefix as usize + new_len;
        let cut_to = stream_start + prefix as usize + old_len;
        let stream_size = prefix as usize + old_len + after.len();
        let mut cuts = vec![stream_start + 100, stream_start + 2000, cut_from + 500,
                            cut_to - 500, cut_to + 300, data.len()];
        cuts.sort_unstable(); cuts.dedup();
        let mut sizes = Vec::new();
        let mut previous = 0usize;
        for cut in cuts { sizes.push(cut - previous); previous = cut; }
        let nodes = [(0u64, sf.len() as u64, 4u32, "CAB-test"),
                     (stream_start as u64, stream_size as u64, 0u32, "CAB-test.resS"),
                     ((stream_start + stream_size) as u64, other.len() as u64, 0u32, "CAB-test.other")];
        // Poor (literal-only) source encoding: every touched block is re-encoded
        // strictly smaller, so the strictly-smaller publication gate passes.
        let bundle = bundle_with_encoder(&data, &nodes, &sizes, 3, [0u8; 16], literal_lz4);
        let root = scratch("bgc-unitystream-compact");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let new_payload = payload_of(new_len, 11);
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &new_payload).unwrap();
        let output = root.join("shrunk.bundle");
        texture_replace(0, 123, &payload_file, &input, &output).unwrap();
        // (a) The source is never modified and only a strictly smaller container
        // is published; the decoded bundle shrank by exactly the removed delta.
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let out = fs::read(&output).unwrap();
        assert!(out.len() < bundle.len(), "published bundle must be strictly smaller");
        let (before_bundle, after_bundle) = (parse(&bundle).unwrap(), parse(&out).unwrap());
        assert_eq!(before_bundle.nodes.len(), after_bundle.nodes.len());
        assert_eq!(after_bundle.decoded + removed, before_bundle.decoded);
        // (b) The .resS node lost exactly the freed tail and the following node
        // moved down by the same delta; paths and flags are unchanged.
        assert_eq!(after_bundle.nodes[1].offset, before_bundle.nodes[1].offset);
        assert_eq!(after_bundle.nodes[1].size + removed, before_bundle.nodes[1].size);
        assert_eq!(after_bundle.nodes[2].offset + removed, before_bundle.nodes[2].offset);
        assert_eq!(after_bundle.nodes[2].size, before_bundle.nodes[2].size);
        for (old, new) in before_bundle.nodes.iter().zip(&after_bundle.nodes) {
            assert_eq!((old.flags, &old.path), (new.flags, &new.path));
        }
        let (mut old_decoder, mut new_decoder) = (NodeDecoder::new(&bundle, &before_bundle),
            NodeDecoder::new(&out, &after_bundle));
        // (c) The SerializedFile object table is byte-stable; the target re-parses
        // with the new declared shape and the later texture re-parses with its
        // offset shifted down by exactly the removed delta.
        let old_node = old_decoder.read(0, before_bundle.nodes[0].size).unwrap();
        let new_node = new_decoder.read(0, after_bundle.nodes[0].size).unwrap();
        let old_inventory = node_texture_inventory(&old_node).unwrap();
        let new_inventory = node_texture_inventory(&new_node).unwrap();
        assert_eq!(old_inventory.object_records.len(), new_inventory.object_records.len());
        for (old, new) in old_inventory.object_records.iter().zip(&new_inventory.object_records) {
            assert_eq!((old.id, old.start, old.size, old.type_index, old.table_start, old.table_end),
                (new.id, new.start, new.size, new.type_index, new.table_start, new.table_end));
        }
        fn fields_of(inventory: &crate::unity_serialized::BundleTextureInventory, id: u64)
            -> &crate::unity_tree::TextureFields {
            &inventory.details.iter().find(|(candidate, _)| *candidate == id).unwrap().1
        }
        let target_fields = fields_of(&new_inventory, 123);
        assert_eq!((target_fields.texture.width, target_fields.texture.height, target_fields.texture.mips), (64, 64, 7));
        assert_eq!(target_fields.texture.inline_bytes, 0);
        assert_eq!((target_fields.texture.stream_offset, target_fields.texture.stream_size), (prefix, new_len as u64));
        assert_eq!(fields_of(&old_inventory, 124).texture.stream_offset, 0);
        assert_eq!(fields_of(&new_inventory, 124).texture.stream_offset, 0);
        assert_eq!(fields_of(&old_inventory, 125).texture.stream_offset, prefix + old_len as u64);
        assert_eq!(fields_of(&new_inventory, 125).texture.stream_offset, prefix + new_len as u64);
        // Every changed SerializedFile byte must lie in one declared patch: the
        // target's shape/size fields or the later texture's offset field.
        let mut allowed = Vec::new();
        for (id, fields) in &new_inventory.details {
            let record = new_inventory.object_records.iter().find(|record| record.id == *id).unwrap();
            let names: &[&str] = if *id == 123 {
                &["m_Width", "m_Height", "m_MipCount", "m_CompleteImageSize", "m_StreamData/size"]
            } else if *id == 125 {
                &["m_StreamData/offset"]
            } else { &[] };
            for name in names {
                let span = fields.spans.get(*name).unwrap();
                allowed.push((record.start as usize + span.start, record.start as usize + span.end));
            }
        }
        for (offset, (old, new)) in old_node.iter().zip(&new_node).enumerate() {
            if old != new {
                assert!(allowed.iter().any(|(start, end)| offset >= *start && offset < *end),
                    "SerializedFile byte {offset} changed outside every declared patch");
            }
        }
        // Read the raw shifted offset field back through its span.
        let record_125 = new_inventory.object_records.iter().find(|record| record.id == 125).unwrap();
        let span_125 = fields_of(&new_inventory, 125).spans.get("m_StreamData/offset").unwrap();
        assert_eq!(read_u64(&new_node, record_125.start as usize + span_125.start, false).unwrap(),
            prefix + new_len as u64, "the later texture's offset field must hold the shifted offset");
        // (d) The .resS node keeps the prefix, holds the new payload, then the
        // shifted suffix exactly; the trailing node is byte-identical.
        let old_stream = old_decoder.read(before_bundle.nodes[1].offset, before_bundle.nodes[1].size).unwrap();
        let new_stream = new_decoder.read(after_bundle.nodes[1].offset, after_bundle.nodes[1].size).unwrap();
        assert_eq!(new_stream.len() as u64 + removed, old_stream.len() as u64);
        assert_eq!(&new_stream[..prefix as usize], &old_stream[..prefix as usize]);
        assert_eq!(&new_stream[prefix as usize..prefix as usize + new_len], &new_payload[..]);
        assert_eq!(&new_stream[prefix as usize + new_len..], &old_stream[prefix as usize + old_len..]);
        assert_eq!(old_decoder.read(before_bundle.nodes[2].offset, before_bundle.nodes[2].size).unwrap(),
            new_decoder.read(after_bundle.nodes[2].offset, after_bundle.nodes[2].size).unwrap());
        // (e) Every streamed texture re-extracts byte-identically except the
        // target, which equals the supplied payload.
        let extract = |decoder: &mut NodeDecoder, bundle: &Bundle, inventory: &crate::unity_serialized::BundleTextureInventory,
                       id: u64| -> Vec<u8> {
            let fields = fields_of(inventory, id);
            decoder.read(bundle.nodes[1].offset + fields.texture.stream_offset, fields.texture.stream_size).unwrap()
        };
        assert_eq!(extract(&mut new_decoder, &after_bundle, &new_inventory, 123), new_payload);
        assert_eq!(extract(&mut new_decoder, &after_bundle, &new_inventory, 124),
            extract(&mut old_decoder, &before_bundle, &old_inventory, 124));
        assert_eq!(extract(&mut new_decoder, &after_bundle, &new_inventory, 125), after);
        assert_eq!(extract(&mut old_decoder, &before_bundle, &old_inventory, 125), after);
        // (f) The payload re-extracts byte-identically through the existing
        // streamed export route. An existing destination is never replaced.
        let dds_path = root.join("check.dds");
        texture_export(0, 123, &output, &dds_path).unwrap();
        let dds = fs::read(&dds_path).unwrap();
        let check = crate::texture::parse(&dds).unwrap();
        assert_eq!((check.width, check.height, check.mipmaps), (64, 64, 7));
        assert_eq!(&dds[check.data_start..], &new_payload[..]);
        assert!(texture_replace(0, 123, &payload_file, &input, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), out);
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn streamed_shrink_zero_pads_when_a_reference_cannot_be_proven() {
        // An unresolved streamed reference (the replace route's unique-claim
        // check skips it, but its range cannot be proven) blocks physical
        // compaction. The zero-padded fallback still runs: the extent keeps its
        // size, the freed tail is zeroed, the decoded bundle is unchanged and a
        // strictly smaller candidate is still published.
        let old_len = chain_len(10, 128, 128);
        let new_len = chain_len(10, 64, 64);
        let old_payload = payload_of(old_len, 7);
        let new_payload = payload_of(new_len, 11);
        let other = payload_of(600, 17);
        let target = streamed_object_at(128, 128, 10, 0, old_len as u32, "CAB-test.resS");
        let stray = streamed_object_at(8, 8, 10, 5000, 1000, "/abs/missing.resS");
        let sf = serialized_objects(&[(123, target), (124, stray)]);
        let mut data = sf.clone();
        data.extend_from_slice(&old_payload);
        data.extend_from_slice(&other);
        let mut cuts = vec![sf.len() + 100, sf.len() + 700, sf.len() + 2900,
                            sf.len() + old_len, data.len()];
        cuts.sort_unstable(); cuts.dedup();
        let mut sizes = Vec::new();
        let mut previous = 0usize;
        for cut in cuts { sizes.push(cut - previous); previous = cut; }
        let nodes = [(0u64, sf.len() as u64, 4u32, "CAB-test"),
                     (sf.len() as u64, old_len as u64, 0u32, "CAB-test.resS"),
                     ((sf.len() + old_len) as u64, other.len() as u64, 0u32, "CAB-test.other")];
        let bundle = bundle_with_encoder(&data, &nodes, &sizes, 3, [0u8; 16], literal_lz4);
        // The plan itself refuses the unresolved reference: this is the blocker
        // that keeps the route on the padded path.
        let parsed = parse(&bundle).unwrap();
        let blockers = plan_stream_compaction(&bundle, &parsed, 0, 123, 1, 0, old_len as u64, new_len).unwrap_err();
        assert!(blockers.iter().any(|blocker| blocker.contains("does not resolve in this bundle")), "{blockers:?}");
        let root = scratch("bgc-unitystream-padded");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin"); fs::write(&payload_file, &new_payload).unwrap();
        let output = root.join("shrunk.bundle");
        texture_replace(0, 123, &payload_file, &input, &output).unwrap();
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        let out = fs::read(&output).unwrap();
        assert!(out.len() < bundle.len(), "published bundle must be strictly smaller");
        let (before, after) = (parse(&bundle).unwrap(), parse(&out).unwrap());
        assert_eq!(before.nodes.len(), after.nodes.len());
        assert_eq!(before.blocks.len(), after.blocks.len());
        assert_eq!(before.decoded, after.decoded, "the fallback must keep the decoded layout");
        assert_eq!((after.nodes[1].offset, after.nodes[1].size), (before.nodes[1].offset, before.nodes[1].size));
        let (mut old_decoder, mut new_decoder) = (NodeDecoder::new(&bundle, &before), NodeDecoder::new(&out, &after));
        let old_node = old_decoder.read(0, before.nodes[0].size).unwrap();
        let new_node = new_decoder.read(0, after.nodes[0].size).unwrap();
        let old_records = node_texture_inventory(&old_node).unwrap();
        let new_records = node_texture_inventory(&new_node).unwrap();
        for (old, new) in old_records.object_records.iter().zip(&new_records.object_records) {
            assert_eq!((old.id, old.start, old.size, old.type_index, old.table_start, old.table_end),
                (new.id, new.start, new.size, new.type_index, new.table_start, new.table_end));
        }
        let (_, fields) = new_records.details.iter().find(|(id, _)| *id == 123).unwrap();
        assert_eq!((fields.texture.width, fields.texture.height, fields.texture.mips), (64, 64, 7));
        assert_eq!(fields.texture.inline_bytes, 0);
        assert_eq!((fields.texture.stream_offset, fields.texture.stream_size), (0, new_len as u64));
        let record = new_records.object_records.iter().find(|record| record.id == 123).unwrap();
        let target_object = &new_node[record.start as usize..(record.start + record.size) as usize];
        for (name, value) in [("m_Width", 64u32), ("m_Height", 64), ("m_MipCount", 7),
                              ("m_CompleteImageSize", new_len as u32), ("m_StreamData/size", new_len as u32)] {
            let span = fields.spans.get(name).unwrap();
            assert_eq!(read_u32(target_object, span.start, false).unwrap(), value, "{name}");
        }
        // The .resS extent holds the new payload followed by zeros, every byte
        // outside the OLD extent is unchanged, and the third node is untouched.
        let old_stream = old_decoder.read(before.nodes[1].offset, before.nodes[1].size).unwrap();
        let new_stream = new_decoder.read(after.nodes[1].offset, after.nodes[1].size).unwrap();
        assert_eq!(&new_stream[..new_len], &new_payload[..]);
        assert!(new_stream[new_len..old_len].iter().all(|byte| *byte == 0), "old extent tail must be zero");
        assert_eq!(&old_stream[old_len..], &new_stream[old_len..], "outside the old extent must be unchanged");
        assert_eq!(old_decoder.read(before.nodes[2].offset, before.nodes[2].size).unwrap(),
            new_decoder.read(after.nodes[2].offset, after.nodes[2].size).unwrap());
        // The payload re-extracts byte-identically through the exported route.
        let dds_path = root.join("check.dds");
        texture_export(0, 123, &output, &dds_path).unwrap();
        let dds = fs::read(&dds_path).unwrap();
        let check = crate::texture::parse(&dds).unwrap();
        assert_eq!((check.width, check.height, check.mipmaps), (64, 64, 7));
        assert_eq!(&dds[check.data_start..], &new_payload[..]);
        assert!(texture_replace(0, 123, &payload_file, &input, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), out);
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stream_compaction_plan_refuses_overlapping_unresolved_and_absent_references() {
        // The replace route's unique-claim check refuses a same-node overlap
        // before the plan runs, so the plan's own fail-closed rules are proven
        // directly: a range that straddles the cut, a range inside the removed
        // region, an unresolvable stream path and a resolved-but-absent target
        // each block compaction with their own reason.
        let old_len = chain_len(10, 128, 128);
        let new_len = chain_len(10, 64, 64);
        let stream = payload_of(old_len as usize + 2048, 7);
        let target = streamed_object_at(128, 128, 10, 64, old_len as u32, "CAB-test.resS");
        let cases: &[(&str, Vec<u8>, &str)] = &[
            ("straddle", streamed_object_at(8, 8, 10, 2700, 500, "CAB-test.resS"),
                "straddles the start of the removed region"),
            ("inside", streamed_object_at(8, 8, 10, 5000, 100, "CAB-test.resS"),
                "lies inside the removed region"),
            ("unresolved", streamed_object_at(8, 8, 10, 5000, 100, "/abs/missing.resS"),
                "does not resolve in this bundle"),
            ("absent", streamed_object_at(8, 8, 10, 5000, 100, "CAB-absent.resS"),
                "is not present in this bundle"),
        ];
        for (name, stray, expected) in cases {
            let sf = serialized_objects(&[(123, target.clone()), (124, stray.clone())]);
            let bundle = bundle_with_serialized_and_stream(&sf, &stream);
            let parsed = parse(&bundle).unwrap();
            let blockers = plan_stream_compaction(&bundle, &parsed, 0, 123, 1, 64, old_len as u64, new_len)
                .expect_err(name);
            assert!(blockers.iter().any(|blocker| blocker.contains(expected)),
                "{name}: expected {expected:?}, got {blockers:?}");
        }
    }
    #[test]
    fn stream_compaction_plan_refuses_more_than_the_streamed_reference_bound() {
        // One target plus MAX_STREAM_REFS other streamed textures exceed the
        // stated bound; the plan refuses instead of looping over an unbounded
        // object set.
        let target = streamed_object_at(8, 8, 10, 0, 1000, "CAB-test.resS");
        let mut objects = vec![(123u64, target)];
        for index in 0..MAX_STREAM_REFS {
            objects.push((124 + index as u64,
                streamed_object_at(8, 8, 10, 1000 + index as u64 * 16, 4, "CAB-test.resS")));
        }
        let sf = serialized_objects(&objects);
        let stream = vec![0u8; 1000 + MAX_STREAM_REFS * 16 + 16];
        let bundle = bundle_with_serialized_and_stream(&sf, &stream);
        let parsed = parse(&bundle).unwrap();
        let blockers = plan_stream_compaction(&bundle, &parsed, 0, 123, 1, 0, 1000, 500).unwrap_err();
        assert!(blockers.iter().any(|blocker| blocker.contains("more than the")), "{blockers:?}");
    }
    #[test]
    fn streamed_replace_refuses_non_shorter_unresolved_aliased_and_misshaped_payloads() {
        let old_len = chain_len(10, 128, 128);
        let new_len = chain_len(10, 64, 64);
        let object = streamed_object_shape(128, 128, 10, "CAB-test.resS");
        let sf = serialized_inline_texture(&object);
        let bundle = bundle_with_serialized_and_stream(&sf, &payload_of(old_len, 7));
        let root = scratch("bgc-unitystream-refuse");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin");
        // The streamed route is shrink-only: equal-length and larger payloads are
        // refused, as are lengths that match no complete halved-shape chain and
        // shapes the repository short-edge policy refuses. Nothing is published.
        let cases = [
            ("same", payload_of(old_len, 3), "strictly shorter"),
            ("larger", payload_of(old_len + 8, 3), "strictly shorter"),
            ("levels", payload_of(new_len - 8, 3), "complete 1x1-terminated"),
            ("odd", payload_of(new_len / 2 + 1, 3), "complete 1x1-terminated"),
            ("thin", payload_of(chain_len(10, 32, 32), 3), "texture policy"),
        ];
        for (name, payload, message) in cases {
            fs::write(&payload_file, &payload).unwrap();
            let destination = root.join(&format!("{name}.bundle"));
            let error = texture_replace(0, 123, &payload_file, &input, &destination).unwrap_err();
            assert!(format!("{error}").contains(message), "{name}: {error}");
            assert!(!destination.exists(), "{name} must not publish");
        }
        assert_eq!(fs::read(&input).unwrap(), bundle, "source must stay unchanged");
        // A stream path with no matching same-bundle node is refused by the shared
        // selection rather than guessed from the host filesystem.
        let missing_object = streamed_object_shape(128, 128, 10, "missing.resS");
        let missing_bundle = bundle_with_serialized(&serialized_inline_texture(&missing_object));
        let missing_input = root.join("missing"); fs::write(&missing_input, &missing_bundle).unwrap();
        fs::write(&payload_file, payload_of(new_len, 3)).unwrap();
        let error = texture_replace(0, 123, &payload_file, &missing_input, &root.join("missing.out")).unwrap_err();
        assert!(format!("{error}").contains("not present"), "{error}");
        assert!(!root.join("missing.out").exists());
        // An extent another retained texture also claims is refused as aliased
        // even though both extents are individually in bounds.
        let target = streamed_object_shape(128, 128, 10, "CAB-test.resS");
        let other = streamed_object_at(8, 8, 4, 5000, 1000, "CAB-test.resS");
        let aliased_sf = serialized_objects(&[(123, target), (124, other)]);
        let aliased = bundle_with_serialized_and_stream(&aliased_sf, &payload_of(old_len, 7));
        let aliased_input = root.join("aliased"); fs::write(&aliased_input, &aliased).unwrap();
        let error = texture_replace(0, 123, &payload_file, &aliased_input, &root.join("aliased.out")).unwrap_err();
        assert!(format!("{error}").contains("aliases"), "{error}");
        assert!(!root.join("aliased.out").exists());
        assert_eq!(fs::read(&aliased_input).unwrap(), aliased);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn streamed_no_gain_replacements_publish_nothing() {
        // With an unresolved reference the physical plan is refused and the
        // zero-padded fallback runs. In a stored-codec bundle that fallback keeps
        // every block at its decoded length, so zeroing the extent cannot shrink
        // the container: the route reports NO_GAIN and leaves the destination
        // absent. The declared extent starts 111 bytes into the .resS node, so
        // the same-offset patch arithmetic and the internal re-extraction check
        // are exercised off the node start.
        let old_len = chain_len(10, 128, 128);
        let offset = 111u64;
        let target = streamed_object_at(128, 128, 10, offset, old_len as u32, "CAB-test.resS");
        let stray = streamed_object_at(8, 8, 10, 5000, 100, "missing.resS");
        let sf = serialized_objects(&[(123, target), (124, stray)]);
        let mut stream = vec![0x5au8; offset as usize];
        stream.extend_from_slice(&payload_of(old_len, 7));
        let bundle = bundle_with_serialized_and_stream(&sf, &stream);
        let root = scratch("bgc-unitystream-nogain");
        let input = root.join("bundle"); fs::write(&input, &bundle).unwrap();
        let payload_file = root.join("payload.bin");
        fs::write(&payload_file, payload_of(chain_len(10, 64, 64), 11)).unwrap();
        let destination = root.join("shrunk.bundle");
        texture_replace(0, 123, &payload_file, &input, &destination).unwrap();
        assert!(!destination.exists(), "NO_GAIN must not publish");
        assert_eq!(fs::read(&input).unwrap(), bundle);
        fs::remove_dir_all(root).unwrap();
    }

    // ---- in-place bundle apply (M1 beta apply step) -----------------------
    /// A bundle with one large inline DXT1 texture, one streamed DXT1 texture
    /// at a nonzero `.resS` offset, one small inline texture already within
    /// every profile cap, and one small streamed texture after the target (a
    /// physical compaction must shift its declared offset even though the apply
    /// skips it as already at cap). Poor LZ4 source blocks mean the verified
    /// rebuild is strictly smaller.
    fn apply_fixture() -> Vec<u8> {
        let old_len = chain_len(10, 128, 128);
        let small_len = chain_len(10, 8, 8);
        let streamed = streamed_object_at(128, 128, 10, 64, old_len as u32, "CAB-test.resS");
        let small_streamed = streamed_object_at(8, 8, 10, (64 + old_len) as u64, small_len as u32, "CAB-test.resS");
        let sf = serialized_objects(&[
            (123, inline_object(128, 128, 10, 8, &payload_of(old_len, 7))),
            (124, streamed),
            (125, inline_object(64, 64, 4, 1, &payload_of(64 * 64 * 4, 31))),
            (126, small_streamed),
        ]);
        let prefix = payload_of(64, 21);
        let after = payload_of(600, 29);
        let stream_start = sf.len();
        let stream_size = prefix.len() + old_len + small_len;
        let mut data = sf.clone();
        data.extend_from_slice(&prefix);
        data.extend_from_slice(&payload_of(old_len, 13));
        data.extend_from_slice(&payload_of(small_len, 17));
        data.extend_from_slice(&after);
        let nodes = [
            (0u64, sf.len() as u64, 4u32, "CAB-test"),
            (stream_start as u64, stream_size as u64, 0u32, "CAB-test.resS"),
            ((stream_start + stream_size) as u64, after.len() as u64, 0u32, "CAB-test.other"),
        ];
        let mut cuts = vec![stream_start + 100, stream_start + 2000,
                            stream_start + stream_size - 500, data.len()];
        cuts.sort_unstable();
        let mut sizes = Vec::new();
        let mut previous = 0usize;
        for cut in cuts { sizes.push(cut - previous); previous = cut; }
        bundle_with_encoder(&data, &nodes, &sizes, 3, [0u8; 16], literal_lz4)
    }

    fn inline_payload_of(node: &[u8], inventory: &crate::unity_serialized::BundleTextureInventory, id: u64) -> Vec<u8> {
        let (_, fields) = inventory.details.iter().find(|(candidate, _)| *candidate == id).unwrap();
        let record = inventory.object_records.iter().find(|record| record.id == id).unwrap();
        let span = fields.spans.get("image data").or_else(|| fields.spans.get("image data/Array")).unwrap();
        let (offset, len) = span.payload.unwrap();
        node[record.start as usize + offset..record.start as usize + offset + len].to_vec()
    }

    fn no_apply_debris(root: &Path) {
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(!name.contains(".bgc-tmp-"), "temporary file left behind: {name}");
                assert!(!name.contains(".bgc-packed-stage-"), "stage file left behind: {name}");
                if entry.file_type().unwrap().is_dir() { dirs.push(entry.path()); }
            }
        }
    }

    use crate::unityfs::apply::{texture_apply, ApplyStatus};
    use std::path::PathBuf;

    #[test]
    fn unity_bundle_apply_backs_up_applies_and_restores_on_a_copy() {
        let root = scratch("bgc-unity-apply-round");
        fs::create_dir(root.join("content")).unwrap();
        let rel = PathBuf::from("content/test.bundle");
        let path = root.join(&rel);
        let bundle = apply_fixture();
        fs::write(&path, &bundle).unwrap();
        let original = fs::read(&path).unwrap();
        let old_stream = chain_len(10, 128, 128);
        let new_stream = chain_len(10, 64, 64);

        let outcomes = texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap();
        let outcome = &outcomes[0];
        assert_eq!(outcome.status, ApplyStatus::Applied, "{outcome:?}");
        assert_eq!(outcome.applied, 2);
        assert_eq!(outcome.removed_decoded, (old_stream - new_stream) as u64);
        assert!(outcome.new_bytes < outcome.old_bytes);

        // The installed bundle re-parses; both targets re-extract at 64x64/7
        // mips; the unrelated node and the small texture payload are
        // byte-identical to the source.
        let installed = fs::read(&path).unwrap();
        assert!(installed.len() < original.len());
        let (before, after) = (parse(&original).unwrap(), parse(&installed).unwrap());
        assert_eq!(before.nodes.len(), after.nodes.len());
        assert_eq!(after.decoded + outcome.removed_decoded, before.decoded);
        assert_eq!(after.nodes[2].offset + outcome.removed_decoded, before.nodes[2].offset);
        let (mut before_decoder, mut after_decoder) =
            (NodeDecoder::new(&original, &before), NodeDecoder::new(&installed, &after));
        let old_node = before_decoder.read(0, before.nodes[0].size).unwrap();
        let new_node = after_decoder.read(0, after.nodes[0].size).unwrap();
        let old_records = node_texture_inventory(&old_node).unwrap();
        let new_records = node_texture_inventory(&new_node).unwrap();
        for (old, new) in old_records.object_records.iter().zip(&new_records.object_records) {
            assert_eq!((old.id, old.start, old.size, old.type_index, old.table_start, old.table_end),
                (new.id, new.start, new.size, new.type_index, new.table_start, new.table_end));
        }
        assert_eq!(inline_payload_of(&old_node, &old_records, 125),
            inline_payload_of(&new_node, &new_records, 125),
            "an unrelated inline payload must stay byte-identical");
        for id in [123u64, 124] {
            let (_, fields) = new_records.details.iter().find(|(candidate, _)| *candidate == id).unwrap();
            assert_eq!((fields.texture.width, fields.texture.height, fields.texture.mips), (64, 64, 7));
        }
        assert_eq!(new_records.details.iter().find(|(id, _)| *id == 123).unwrap().1.texture.inline_bytes, new_stream);
        assert_eq!(new_records.details.iter().find(|(id, _)| *id == 124).unwrap().1.texture.stream_size, new_stream as u64);
        // The streamed target re-extracts through the shared selection at its
        // new declared shape and decodes at 64x64.
        let mut check_decoder = NodeDecoder::new(&installed, &after);
        let located = locate_texture(0, 124, &after, &mut check_decoder).unwrap();
        let LocatedStorage::Streamed { target, offset, size } = located.storage else { panic!("streamed storage expected") };
        let payload = check_decoder.read(after.nodes[target].offset + offset, size).unwrap();
        assert_eq!(payload.len(), new_stream);
        let decoded = image_dds::Surface { width: 64, height: 64, depth: 1, layers: 1,
            mipmaps: 7, image_format: located.format, data: &payload }.decode_rgba8().unwrap();
        assert_eq!((decoded.width, decoded.height), (64, 64));
        // The skipped small streamed texture keeps its bytes but its declared
        // offset was shifted down by exactly the physically removed delta.
        let small = locate_texture(0, 126, &after, &mut check_decoder).unwrap();
        let LocatedStorage::Streamed { offset: small_offset, size: small_size, .. } = small.storage else {
            panic!("streamed storage expected for the small texture")
        };
        assert_eq!(small_offset, 64 + new_stream as u64);
        assert_eq!(small_size, chain_len(10, 8, 8) as u64);
        // The `.resS` node keeps its prefix, holds the new payload, and lost the
        // freed tail; the trailing node is byte-identical.
        let new_stream_bytes = after_decoder.read(after.nodes[1].offset, after.nodes[1].size).unwrap();
        let old_stream_bytes = before_decoder.read(before.nodes[1].offset, before.nodes[1].size).unwrap();
        assert_eq!(new_stream_bytes.len() as u64 + outcome.removed_decoded, before.nodes[1].size);
        assert_eq!(&new_stream_bytes[..64], &old_stream_bytes[..64]);
        assert_eq!(before_decoder.read(before.nodes[2].offset, before.nodes[2].size).unwrap(),
            after_decoder.read(after.nodes[2].offset, after.nodes[2].size).unwrap());

        // The restorable original is byte-identical and the checksum sidecar
        // matches the installed bytes, so `assets restore` accepts the state.
        let backup = root.join(".bgc-assets-backup").join(&rel);
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert!(crate::assets::matches_recorded_checksum(&root, &rel).unwrap());
        no_apply_debris(&root);
        // A second pass with the backup present is refused as already optimized.
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::AlreadyOptimized);
        assert_eq!(fs::read(&path).unwrap(), installed);

        // `assets restore` returns the copy byte-identically.
        crate::assets::run("restore", "native", 0, &root).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!backup.exists());
        // Re-apply, then `assets finalize` keeps the transformed file and drops
        // the restore copy; a further pass is a no-op with no backup retained.
        assert_eq!(texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap()[0].status, ApplyStatus::Applied);
        let transformed = fs::read(&path).unwrap();
        crate::assets::run("finalize", "native", 0, &root).unwrap();
        assert_eq!(fs::read(&path).unwrap(), transformed);
        assert!(!backup.exists());
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::NoGain, "{outcome:?}");
        assert_eq!(fs::read(&path).unwrap(), transformed);
        assert!(!backup.exists(), "a NO_GAIN bundle must not retain a backup");
        no_apply_debris(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unity_bundle_apply_refuses_hashes_and_addressables_layouts() {
        let root = scratch("bgc-unity-apply-refusals");
        // A nonzero info-section hash cannot be recomputed: refuse, leave the
        // bytes untouched and create no recovery state.
        let old_len = chain_len(10, 128, 128);
        let sf = serialized_inline_texture(&inline_object(128, 128, 10, 8, &payload_of(old_len, 7)));
        let hashed = bundle_with_blocks(&sf, &[(0, sf.len() as u64, 4, "CAB-test")], &[sf.len()], 0, [0x5a; 16]);
        let rel = PathBuf::from("hashed.bundle");
        fs::write(root.join(&rel), &hashed).unwrap();
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::Refused, "{outcome:?}");
        assert!(outcome.refusal.as_deref().unwrap().contains("nonzero"), "{:?}", outcome.refusal);
        assert_eq!(fs::read(root.join(&rel)).unwrap(), hashed);
        assert!(!root.join(".bgc-assets-backup").exists());
        // An `aa` content component is treated as an Addressables layout: the
        // catalogue may carry a bundle hash this build cannot recompute.
        let clean = apply_fixture();
        fs::create_dir_all(root.join("aa/StandaloneWindows64")).unwrap();
        fs::write(root.join("aa/StandaloneWindows64/game.bundle"), &clean).unwrap();
        let aa_rel = PathBuf::from("aa/StandaloneWindows64/game.bundle");
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&aa_rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::Refused, "{outcome:?}");
        assert!(outcome.refusal.as_deref().unwrap().contains("aa"), "{:?}", outcome.refusal);
        assert_eq!(fs::read(root.join(&aa_rel)).unwrap(), clean);
        // A catalogue file in an ancestor directory without an `aa` component
        // is refused for the same reason.
        fs::create_dir_all(root.join("content/deep")).unwrap();
        fs::write(root.join("content/catalog.json"), b"{}").unwrap();
        fs::write(root.join("content/deep/game.bundle"), &clean).unwrap();
        let cat_rel = PathBuf::from("content/deep/game.bundle");
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&cat_rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::Refused, "{outcome:?}");
        assert!(outcome.refusal.as_deref().unwrap().contains("catalogue"), "{:?}", outcome.refusal);
        assert_eq!(fs::read(root.join(&cat_rel)).unwrap(), clean);
        assert!(!root.join(".bgc-assets-backup").exists());
        no_apply_debris(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unity_bundle_apply_fails_closed_on_unverifiable_input() {
        let root = scratch("bgc-unity-apply-unverifiable");
        let rel = PathBuf::from("broken.bundle");
        let bundle = apply_fixture();
        fs::write(root.join(&rel), &bundle[..bundle.len() / 3]).unwrap();
        let original = fs::read(root.join(&rel)).unwrap();
        assert!(texture_apply(64, 1, &root, std::slice::from_ref(&rel)).is_err());
        assert_eq!(fs::read(root.join(&rel)).unwrap(), original);
        assert!(!root.join(".bgc-assets-backup").exists());
        no_apply_debris(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unity_bundle_apply_holds_the_game_lock_and_reports_no_gain() {
        let root = scratch("bgc-unity-apply-lock");
        // The stored-codec bundle keeps its object extent, so the rebuild is
        // not strictly smaller: nothing may be written and no backup retained.
        let old_len = chain_len(10, 128, 128);
        let sf = serialized_inline_texture(&inline_object(128, 128, 10, 8, &payload_of(old_len, 7)));
        let stored = bundle_with_serialized(&sf);
        let rel = PathBuf::from("stored.bundle");
        fs::write(root.join(&rel), &stored).unwrap();
        // The same mutation guard `assets apply`/`restore` hold protects it.
        let held = crate::assets::lock_game_for_mutation(&root).unwrap();
        assert_eq!(texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap_err().kind(),
            io::ErrorKind::WouldBlock);
        drop(held);
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::NoGain, "{outcome:?}");
        assert_eq!(fs::read(root.join(&rel)).unwrap(), stored);
        assert!(!root.join(".bgc-assets-backup").exists());
        // A shrink whose zero-tail precondition cannot be proven is skipped
        // with its reason; the source bytes and recovery state stay untouched.
        let mut stale = Vec::new();
        for value in [128u32, 128, old_len as u32, 10, 8, 1, 2] { stale.extend_from_slice(&value.to_le_bytes()); }
        stale.extend_from_slice(&(old_len as u32).to_le_bytes());
        stale.extend_from_slice(&payload_of(old_len, 7));
        stale.extend_from_slice(&0xdead_beefu64.to_le_bytes());
        stale.extend_from_slice(&0u32.to_le_bytes());
        stale.extend_from_slice(&0u32.to_le_bytes());
        while stale.len() % 4 != 0 { stale.push(0); }
        let stale_bundle = bundle_with_serialized(&serialized_inline_texture(&stale));
        let stale_rel = PathBuf::from("stale.bundle");
        fs::write(root.join(&stale_rel), &stale_bundle).unwrap();
        let outcome = &texture_apply(64, 1, &root, std::slice::from_ref(&stale_rel)).unwrap()[0];
        assert_eq!(outcome.status, ApplyStatus::NoGain);
        assert!(outcome.skips.iter().any(|skip| skip.contains("every byte after")), "{:?}", outcome.skips);
        assert_eq!(fs::read(root.join(&stale_rel)).unwrap(), stale_bundle);
        assert!(!root.join(".bgc-assets-backup").exists());
        no_apply_debris(&root);
        fs::remove_dir_all(root).unwrap();
    }
}
