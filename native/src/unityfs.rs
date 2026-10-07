//! Export-only, lossless UnityFS LZ4 recompression. No resource extraction,
//! codec changes, mip removal, or edits to Addressables catalogs/CRC records.
//! Layout reference: AssetStudio BundleFile.cs (format documentation only).
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
    let bundle = parse(b)?;
    let mut info = bundle.info.clone();
    let mut payload = Vec::new();
    let mut changed = 0;
    for block in &bundle.blocks {
        super::cancelled()?;
        let source = &b[block.offset..block.offset + block.encoded];
        let decoded = decode(source, block.decoded, (block.flags & 0x3f) as u32)?;
        let encoded = if block.flags & 0x3f == 0 { source.to_vec() } else {
            let candidate = compress(&decoded, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?;
            super::cancelled()?;
            if candidate.len() < source.len() {
                if decode(&candidate, block.decoded, (block.flags & 0x3f) as u32)? != decoded {
                    return Err(invalid("UnityFS block failed round-trip verification"));
                }
                changed += 1; candidate
            } else { source.to_vec() }
        };
        info[block.size_field..block.size_field + 4].copy_from_slice(&(encoded.len() as u32).to_be_bytes());
        payload.extend_from_slice(&encoded);
    }
    // Do not change any codec IDs or flags, including the original LZ4 vs HC ID.
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
    if check.info != info || check.flags != bundle.flags || check.decoded != bundle.decoded {
        return Err(invalid("rebuilt UnityFS layout failed verification"));
    }
    // Check the finished container, not just individual candidate buffers.
    for (old, new) in bundle.blocks.iter().zip(&check.blocks) {
        super::cancelled()?;
        if decode(&b[old.offset..old.offset + old.encoded], old.decoded, (old.flags & 0x3f) as u32)?
            != decode(&out[new.offset..new.offset + new.encoded], new.decoded, (new.flags & 0x3f) as u32)? {
            return Err(invalid("rebuilt UnityFS content differs"));
        }
    }
    Ok((out, changed, bundle))
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
}
