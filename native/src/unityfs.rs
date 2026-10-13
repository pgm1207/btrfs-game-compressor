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
    rebuild_patched(b, None)
}

/// Container rebuild with an optional byte-range patch in the decoded bundle
/// address space. `None` is the original lossless recompression. `Some(patch)`
/// is used by the detached texture replace: only blocks that intersect the patch
/// are decoded and re-encoded (with their original codec, even when that grows
/// them); every other block keeps its source bytes verbatim. The patch must not
/// change total decoded lengths, so the block table's uncompressed sizes, node
/// extents and the metadata hash field stay valid by construction. Final
/// verification compares each rebuilt block against the original decoded content
/// with the patch applied, so unrelated data is still proven unchanged.
fn rebuild_patched(b: &[u8], patch: Option<(usize, &[u8])>) -> io::Result<(Vec<u8>, usize, Bundle)> {
    let bundle = parse(b)?;
    let patch = match patch {
        Some((start, bytes)) => {
            let end = start.checked_add(bytes.len()).ok_or_else(|| invalid("UnityFS patch extent overflow"))?;
            if end > bundle.decoded as usize { return Err(invalid("UnityFS patch exceeds decoded bundle data")); }
            Some((start, end, bytes))
        }
        None => None,
    };
    let mut info = bundle.info.clone();
    let mut payload = Vec::new();
    let mut changed = 0;
    let mut block_start = 0usize;
    for block in &bundle.blocks {
        super::cancelled()?;
        let block_end = block_start.checked_add(block.decoded).ok_or_else(|| invalid("UnityFS block extent overflow"))?;
        let source = &b[block.offset..block.offset + block.encoded];
        let codec = (block.flags & 0x3f) as u32;
        let encoded = if let Some((patch_start, patch_end, patch_bytes)) = patch {
            let first = patch_start.max(block_start);
            let last = patch_end.min(block_end);
            if first < last {
                let mut decoded = decode(source, block.decoded, codec)?;
                let destination = first - block_start;
                let offset = first - patch_start;
                decoded[destination..destination + (last - first)]
                    .copy_from_slice(&patch_bytes[offset..offset + (last - first)]);
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
            } else {
                // Untouched blocks keep their original compressed bytes exactly.
                source.to_vec()
            }
        } else {
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
    // original block is decoded and the patch is applied before comparison, so
    // patched content is verified and every other byte is required unchanged.
    let mut block_start = 0usize;
    for (old, new) in bundle.blocks.iter().zip(&check.blocks) {
        super::cancelled()?;
        let mut expected = decode(&b[old.offset..old.offset + old.encoded], old.decoded, (old.flags & 0x3f) as u32)?;
        if let Some((start, end, patch_bytes)) = patch {
            let block_end = block_start + old.decoded;
            let first = start.max(block_start);
            let last = end.min(block_end);
            if first < last {
                expected[first - block_start..last - block_start]
                    .copy_from_slice(&patch_bytes[first - start..last - start]);
            }
        }
        let actual = decode(&out[new.offset..new.offset + new.encoded], new.decoded, (new.flags & 0x3f) as u32)?;
        if expected != actual { return Err(invalid("rebuilt UnityFS content differs")); }
        block_start += old.decoded;
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
    let slice = decoder.read(node.offset, node.size)?;
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
fn write_u32(bytes: &mut [u8], offset: usize, value: u32, big: bool) -> io::Result<()> {
    let end = offset.checked_add(4).ok_or_else(|| invalid("Unity field offset overflow"))?;
    let field = bytes.get_mut(offset..end).ok_or_else(|| invalid("truncated Unity field"))?;
    field.copy_from_slice(&if big { value.to_be_bytes() } else { value.to_le_bytes() });
    Ok(())
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

/// Development-only detached replacement of one inline Unity SerializedFile
/// Texture2D payload, published as a NEW bundle file. Selection, identity and
/// storage rules are exactly the export route's. Only a payload whose byte
/// length equals the known mip-chain length for the unchanged width/height/mip
/// count/format is accepted; dimension/mip changes and streamed extents are
/// refused because relocating objects and `.resS` ranges is not implemented.
/// The SerializedFile length prefix and object-table size field are updated
/// (necessarily to their existing values under the same-length rule) and the
/// touched blocks are re-encoded with the source codec. A nonzero
/// uncompressed-data hash field is refused because its algorithm is unproven;
/// an all-zero field is preserved byte-for-byte. No CRC, CAB name, Addressables
/// catalogue or atlas reference is rebuilt, and the artifact is not runtime
/// validated. The source is never modified.
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
    let LocatedStorage::Inline { span_start, payload_start, payload_len } = located.storage else {
        return Err(invalid("Unity texture replace requires inline storage; streamed extents are not relocated"));
    };
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
    let prefix = read_u32(&located.slice, span_start, located.big)?;
    if prefix as usize != payload_len {
        return Err(invalid("Unity inline image length prefix cannot be proven"));
    }
    // Only the exact known mip-chain length is accepted. For one width/height/
    // mip-count/format the chain length is a single value, so a different length
    // is refused: moving the object would shift every later object offset, and
    // the alignment/padding rules for that have not been proven.
    let mut payload_source = Source::open(payload_path, MAX_TEXTURE_BUNDLE, MAX_TEXTURE_BUNDLE + 16)?;
    let new_len = payload_source.len();
    if new_len != located.expected {
        return Err(invalid(&format!(
            "Unity replacement payload must be exactly {} bytes for the unchanged {}x{} format {} mip chain ({} bytes given; dimension, format and mip-count changes are not supported)",
            located.expected, located.fields.texture.width, located.fields.texture.height,
            located.fields.texture.mips, new_len)));
    }
    let new_payload = payload_source.read_at(0, new_len as usize)?;
    // Format bookkeeping: the length prefix and the object table entry's size
    // are computed exactly as the container requires. Under the same-length rule
    // both updates reproduce the existing values; anything else is refused
    // instead of moving offsets.
    let new_object_size = located.record.size - payload_len as u64 + new_len as u64;
    if new_object_size != located.record.size {
        return Err(invalid("Unity texture replace refuses a payload length that would move object offsets"));
    }
    let payload_at = payload_start;
    let patch_start = located.node_offset as usize + payload_at;
    let mut expected_node = located.slice.clone();
    expected_node[payload_at..payload_at + new_len as usize].copy_from_slice(&new_payload);
    write_u32(&mut expected_node, span_start, new_len as u32, located.big)?;
    write_u32(&mut expected_node, size_field as usize, new_object_size as u32, located.big)?;
    let (candidate, _changed, _) = rebuild_patched(&bytes, Some((patch_start, &new_payload)))?;
    // (1) The candidate must re-parse as a UnityFS bundle with the same layout.
    let check_bundle = parse(&candidate)?;
    if check_bundle.flags != bundle.flags || check_bundle.nodes.len() != bundle.nodes.len()
        || check_bundle.blocks.len() != bundle.blocks.len() {
        return Err(invalid("Unity texture replace changed the bundle layout"));
    }
    // (2) The payload must re-extract identically through the same selection, and
    // the whole replaced node must equal the spliced expectation.
    let mut check_decoder = NodeDecoder::new(&candidate, &check_bundle);
    let check = locate_texture(node_index, path_id, &check_bundle, &mut check_decoder)?;
    if check.version != located.version || check.big != located.big
        || check.expected != located.expected
        || check.fields.texture.width != located.fields.texture.width
        || check.fields.texture.height != located.fields.texture.height
        || check.fields.texture.format != located.fields.texture.format
        || check.fields.texture.mips != located.fields.texture.mips
        || check.fields.texture.inline_bytes != located.fields.texture.inline_bytes {
        return Err(invalid("Unity texture replace changed the located texture identity"));
    }
    let LocatedStorage::Inline { payload_start: checked_start, payload_len: checked_len, .. } = check.storage else {
        return Err(invalid("Unity texture replace did not produce inline storage"));
    };
    if checked_len as u64 != new_len
        || &check.slice[checked_start..checked_start + checked_len] != &new_payload[..]
        || check.slice != expected_node {
        return Err(invalid("Unity texture replace payload failed re-extraction"));
    }
    // (3) Every other node's decoded bytes must be unchanged, node by node.
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
    // Publish the fully verified candidate. NO_GAIN is a container-size note only:
    // replacement does not require the rebuilt bundle to be smaller.
    let status = if candidate.len() < bytes.len() { "EXPORTED" } else { "NO_GAIN" };
    super::detached_export::publish(output, &candidate, || source.unchanged())?;
    println!("UNITYFS_TEXTURE_REPLACE|{node_index}|{path_id}|{payload_len}|{new_len}|{}|{}|{status}",
        bundle.nodes.len(), bundle.blocks.len());
    eprintln!("Experimental detached Unity texture replace: source unchanged; one inline payload copied verbatim with no re-encode. NO_GAIN means the published bundle is not smaller than the source and refers to container size only. Only an all-zero uncompressed-data hash field is accepted and it is preserved, not recomputed; no CRC/CAB-name/Addressables-catalogue rebuild, no .resS relocation and no runtime validation. Research artifact only.");
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
        for v in [width, height, format, mips, 1, 2] { o.extend_from_slice(&v.to_le_bytes()); }
        o.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        o.extend_from_slice(payload);
        o.extend_from_slice(&0u64.to_le_bytes()); // stream offset
        o.extend_from_slice(&0u32.to_le_bytes()); // stream size (inline only)
        o.extend_from_slice(&0u32.to_le_bytes()); // stream path length
        while o.len() % 4 != 0 { o.push(0); }
        o
    }
    /// One v22 little-endian SerializedFile with a single Texture2D object (id 123).
    fn serialized_inline_texture(object: &[u8]) -> Vec<u8> {
        let (raw, strings) = texture_tree();
        let mut m = b"2022.3.0f1\0".to_vec();
        put32(&mut m, 19); m.push(1); put32(&mut m, 1);
        put32(&mut m, 28); m.push(0); m.extend([255; 2]); m.extend([0; 16]);
        put32(&mut m, (raw.len() / 32) as u32); put32(&mut m, strings.len() as u32);
        m.extend(&raw); m.extend(&strings);
        put32(&mut m, 0); // dependencies
        put32(&mut m, 1); while m.len() % 4 != 0 { m.push(0); }
        put64(&mut m, 123); put64(&mut m, 0); put32(&mut m, object.len() as u32); put32(&mut m, 0);
        for _ in 0..3 { put32(&mut m, 0); } m.push(0);
        let data = (48 + m.len() as u64).div_ceil(16) * 16;
        let mut sf = vec![0u8; 48];
        sf[8..12].copy_from_slice(&22u32.to_be_bytes());
        sf[20..24].copy_from_slice(&(m.len() as u32).to_be_bytes());
        sf[24..32].copy_from_slice(&(data + object.len() as u64).to_be_bytes());
        sf[32..40].copy_from_slice(&data.to_be_bytes());
        sf.extend(&m); sf.resize(data as usize, 0); sf.extend(object);
        sf
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
        for v in [8u32, 8, 4, 1, 1, 2] { o.extend_from_slice(&v.to_le_bytes()); }
        o.extend_from_slice(&0u32.to_le_bytes()); // inline length 0
        o.extend_from_slice(&0u64.to_le_bytes()); // stream offset 0
        o.extend_from_slice(&size.to_le_bytes()); // stream size
        o.extend_from_slice(&(path.len() as u32).to_le_bytes());
        o.extend_from_slice(path.as_bytes());
        while o.len() % 4 != 0 { o.push(0); }
        o
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
        for v in [8u32, 8, 4, 1, 1, 2] { streamed.extend_from_slice(&v.to_le_bytes()); }
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
        assert_eq!(block_sizes.iter().sum::<usize>(), data.len());
        let encode = |chunk: &[u8]| -> Vec<u8> {
            if codec == 0 { chunk.to_vec() }
            else { compress(chunk, Some(CompressionMode::HIGHCOMPRESSION(9)), false).unwrap() }
        };
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
        let metadata = if codec == 0 { info.clone() } else {
            compress(&info, Some(CompressionMode::HIGHCOMPRESSION(9)), false).unwrap()
        };
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
        let bundle = bundle_with_serialized(&sf);
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
        let bundle = bundle_with_blocks(&data, &nodes, &sizes, 3, [0u8; 16]);
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
        // (c) A streamed texture has no inline payload to replace.
        let stream: Vec<u8> = (0..256u32).map(|i| (i * 3 % 251) as u8).collect();
        let streamed_sf = serialized_inline_texture(&streamed_object("CAB-test.resS", 256));
        let streamed = bundle_with_serialized_and_stream(&streamed_sf, &stream);
        let streamed_input = root.join("streamed"); fs::write(&streamed_input, &streamed).unwrap();
        let error = texture_replace(0, 123, &payload_file, &streamed_input, &root.join("streamed.out")).unwrap_err();
        assert!(format!("{error}").contains("inline storage"), "{error}");
        // (d) A non-bundle input fails closed, and the source files stay untouched.
        let garbage = root.join("garbage"); fs::write(&garbage, b"not a UnityFS bundle").unwrap();
        assert!(texture_replace(0, 123, &payload_file, &garbage, &root.join("garbage.out")).is_err());
        assert_eq!(fs::read(&short_input).unwrap(), short_bundle);
        assert_eq!(fs::read(&hashed_input).unwrap(), hashed);
        assert_eq!(fs::read(&streamed_input).unwrap(), streamed);
        fs::remove_dir_all(root).unwrap();
    }
}
