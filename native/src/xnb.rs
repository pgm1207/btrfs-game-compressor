//! XNB header inventory, a development-only v5 Texture2D metadata reader,
//! and a detached experimental BC texture export.
use std::{fs, io::{self, Read, Write}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug, PartialEq, Eq)]
struct Header {
    target: u8,
    version: u8,
    flags: u8,
    size: u32,
}

fn parse(bytes: &[u8], actual_size: u64) -> io::Result<Header> {
    if bytes.len() < 10 || &bytes[..3] != b"XNB" {
        return Err(invalid("truncated or invalid XNB header"));
    }
    let target = bytes[3];
    let version = bytes[4];
    let flags = bytes[5];
    let size = u32::from_le_bytes(bytes[6..10].try_into().unwrap());
    if !target.is_ascii_lowercase() || !(4..=6).contains(&version) || flags & !0xc1 != 0
        || flags & 0xc0 == 0xc0 || size as u64 != actual_size || size < 10 {
        return Err(invalid("unsupported or inconsistent XNB header"));
    }
    Ok(Header { target, version, flags, size })
}

/// Cheap signature/header gate for directory inventory; no output and no payload reads.
pub fn plausible(bytes: &[u8], actual_size: u64) -> bool {
    parse(bytes, actual_size).is_ok()
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(invalid("XNB audit requires a regular file"));
    }
    let mut bytes = [0u8; 10];
    file.read_exact(&mut bytes)?;
    let h = parse(&bytes, before.len())?;
    let after = file.metadata()?;
    if before.len() != after.len() || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec() || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec() {
        return Err(invalid("XNB source changed during audit"));
    }
    let compression = match h.flags & 0xc0 {
        0 => "none", 0x40 => "LZ4", 0x80 => "LZX", _ => unreachable!(),
    };
    println!("XNB_HEADER|{}|{}|{}|{}|{}|{}", h.target as char, h.version, h.flags,
        compression, h.size, u8::from(h.flags & 1 != 0));
    eprintln!("Read-only XNB header inventory; content-reader IDs, decoded payloads, and texture/audio metadata are not inspected.");
    Ok(())
}

const MAX_READERS: u32 = 128;
const MAX_READER_NAME: u32 = 4096;
const TEXTURE_AUDIT_BUDGET: u64 = 1024 * 1024;

struct ContentReader {
    name: String, version: i32,
    record_start: u64, name_offset: u64, name_size: u32, version_offset: u64,
}
struct Mip { level: u32, width: u32, height: u32, length_offset: u64, offset: u64, size: u64 }
struct ContentInventory {
    header: Header,
    readers: Vec<ContentReader>,
    shared: Option<u32>,
    root: Option<u32>,
    texture: Option<(u32, u32, u32, u32)>,
    mips: Vec<Mip>,
    status: &'static str,
    table_end: Option<u64>,
    root_span: Option<(u64, u64)>,
    texture_offset: Option<u64>,
}

/// Nonnegative .NET-style 7-bit integers only; counts/indices are never signed.
/// Reject overlong encodings and a fifth byte beyond Int32::MAX.
fn seven_bit(source: &mut super::audit_io::Source, offset: &mut u64) -> io::Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let b = source.u8(offset)?;
        if shift == 28 && b > 7 { return Err(invalid("invalid XNB 7-bit integer")); }
        value |= ((b & 0x7f) as u32) << shift;
        if b & 0x80 == 0 {
            if shift != 0 && value < (1u32 << shift) {
                return Err(invalid("overlong XNB 7-bit integer"));
            }
            return Ok(value);
        }
    }
    Err(invalid("unterminated XNB 7-bit integer"))
}

fn known_texture_reader(reader: &ContentReader) -> bool {
    // Assembly-qualified names are data, never loaded/reflected. Generic or
    // custom readers must not match a suffix/substring of this allowlist.
    if reader.version != 0 { return false; }
    let mut parts = reader.name.split(',');
    if parts.next().map(str::trim) != Some("Microsoft.Xna.Framework.Content.Texture2DReader") {
        return false;
    }
    let Some(assembly) = parts.next().map(str::trim) else {
        // MonoGame also registers the exact unqualified built-in reader name.
        // This is not a suffix match or a generic/custom reader allowance.
        return true;
    };
    if !matches!(assembly,
        "Microsoft.Xna.Framework" | "Microsoft.Xna.Framework.Graphics"
        | "MonoGame.Framework" | "FNA") { return false; }
    let mut qualifiers = std::collections::BTreeSet::new();
    for part in parts {
        let Some((key, value)) = part.trim().split_once('=') else { return false; };
        if !qualifiers.insert(key) { return false; }
        let valid = match key {
            "Version" => value.split('.').count() == 4
                && value.split('.').all(|v| !v.is_empty() && v.len() <= 5
                    && v.bytes().all(|b| b.is_ascii_digit()) && v.parse::<u16>().is_ok()),
            "Culture" => value == "neutral",
            "PublicKeyToken" => value == "null"
                || value.len() == 16 && value.bytes().all(|b| b.is_ascii_hexdigit()),
            _ => false,
        };
        if !valid { return false; }
    }
    true
}

fn texture_layout(format: u32) -> Option<super::audit_io::Layout> {
    use super::audit_io::Layout;
    Some(match format {
        0 => Layout::pixels(4), // v5 Color; ordering/alpha not inferred here.
        4 => Layout::bc(8),
        5 | 6 => Layout::bc(16),
        _ => return None,
    })
}

/// Rebuild the exact supported single-mip root while retaining all bytes before
/// its Texture2D header. A byte-identical source rebuild is required before an
/// altered surface is accepted.
fn rebuild_single_mip(bytes: &[u8], texture_offset: usize, format: u32,
    width: u32, height: u32, payload: &[u8]) -> io::Result<Vec<u8>> {
    let prefix = bytes.get(..texture_offset).ok_or_else(|| invalid("XNB texture offset is out of bounds"))?;
    let payload_len = u32::try_from(payload.len()).map_err(|_| invalid("XNB mip exceeds format limit"))?;
    let mut result = prefix.to_vec();
    for value in [format, width, height, 1, payload_len] {
        result.extend_from_slice(&value.to_le_bytes());
    }
    result.extend_from_slice(payload);
    let total = u32::try_from(result.len()).map_err(|_| invalid("XNB rebuilt file exceeds format limit"))?;
    result[6..10].copy_from_slice(&total.to_le_bytes());
    Ok(result)
}

/// The dependency's BC1 encoder emits opaque four-color blocks. Restore the
/// format's one-bit alpha mode for blocks whose resized pixels need it.
fn encode_bc1_alpha_blocks(data: &mut [u8], image: &image::RgbaImage) -> io::Result<()> {
    let (width, height) = image.dimensions();
    let blocks_w = width.div_ceil(4) as usize;
    let blocks_h = height.div_ceil(4) as usize;
    if data.len() != blocks_w * blocks_h * 8 { return Err(invalid("BC1 encoded block count differs from image")); }
    let rgb565 = |value: u16| -> [i32; 3] {
        [(((value >> 11) & 31) as i32 * 255 + 15) / 31,
         (((value >> 5) & 63) as i32 * 255 + 31) / 63,
         ((value & 31) as i32 * 255 + 15) / 31]
    };
    for by in 0..blocks_h {
        for bx in 0..blocks_w {
            if !(0..4).any(|y| (0..4).any(|x| {
                let px = bx as u32 * 4 + x;
                let py = by as u32 * 4 + y;
                px < width && py < height && image.get_pixel(px, py).0[3] < 128
            })) { continue; }
            let block = &mut data[(by * blocks_w + bx) * 8..(by * blocks_w + bx + 1) * 8];
            let mut endpoint0 = u16::from_le_bytes([block[0], block[1]]);
            let mut endpoint1 = u16::from_le_bytes([block[2], block[3]]);
            if endpoint0 > endpoint1 { std::mem::swap(&mut endpoint0, &mut endpoint1); }
            let c0 = rgb565(endpoint0);
            let c1 = rgb565(endpoint1);
            let palette = [c0, c1, [(c0[0] + c1[0]) / 2, (c0[1] + c1[1]) / 2, (c0[2] + c1[2]) / 2]];
            let mut indices = 0u32;
            for y in 0..4 {
                for x in 0..4 {
                    let px = bx as u32 * 4 + x;
                    let py = by as u32 * 4 + y;
                    if px >= width || py >= height { continue; }
                    let pixel = image.get_pixel(px, py).0;
                    let index = if pixel[3] < 128 { 3 } else {
                        (0..3).min_by_key(|&i| (0..3).map(|channel| {
                            let delta = pixel[channel] as i32 - palette[i][channel];
                            delta * delta
                        }).sum::<i32>()).unwrap()
                    };
                    indices |= (index as u32) << (2 * (y * 4 + x));
                }
            }
            block[..2].copy_from_slice(&endpoint0.to_le_bytes());
            block[2..4].copy_from_slice(&endpoint1.to_le_bytes());
            block[4..8].copy_from_slice(&indices.to_le_bytes());
        }
    }
    Ok(())
}

fn content_inventory(source: &mut super::audit_io::Source) -> io::Result<ContentInventory> {
    let bytes = source.read_at(0, 10)?;
    let header = parse(&bytes, source.len())?;
    let mut result = ContentInventory { header, readers: Vec::new(), shared: None,
        root: None, texture: None, mips: Vec::new(), status: "UNSUPPORTED_VERSION",
        table_end: None, root_span: None, texture_offset: None };
    if result.header.version != 5 { return Ok(result); }
    if result.header.flags & 0xc0 != 0 {
        // Compressed content has an additional size field. Inspect that field,
        // but do not send bytes to an arbitrary frame/block decompressor.
        if result.header.size <= 14 { return Err(invalid("XNB compressed payload is empty or truncated")); }
        let bytes = source.read_at(10, 4)?;
        if super::audit_io::le32(&bytes, 0)? == 0 {
            return Err(invalid("invalid XNB declared decoded length"));
        }
        result.status = "COMPRESSED_PAYLOAD_NOT_DECODED";
        return Ok(result);
    }
    if !matches!(result.header.target, b'w' | b'd') {
        result.status = "UNSUPPORTED_TARGET";
        return Ok(result);
    }
    let mut offset = 10u64;
    let count = seven_bit(source, &mut offset)?;
    if count == 0 || count > MAX_READERS { return Err(invalid("XNB reader count exceeds audit subset")); }
    for _ in 0..count {
        super::cancelled()?;
        let record_start = offset;
        let len = seven_bit(source, &mut offset)?;
        if len == 0 || len > MAX_READER_NAME { return Err(invalid("XNB reader name exceeds audit limit")); }
        let name_offset = offset;
        let bytes = source.read_at(offset, len as usize)?;
        offset = offset.checked_add(len as u64).ok_or_else(|| invalid("XNB reader offset overflow"))?;
        let name = std::str::from_utf8(&bytes).map_err(|_| invalid("invalid XNB reader UTF-8"))?.to_owned();
        let version_offset = offset;
        let version = source.u32(&mut offset)? as i32;
        result.readers.push(ContentReader { name, version, record_start,
            name_offset, name_size: len, version_offset });
    }
    result.table_end = Some(offset);
    let shared = seven_bit(source, &mut offset)?;
    result.shared = Some(shared);
    let root_start = offset;
    let root = seven_bit(source, &mut offset)?;
    if root > count { return Err(invalid("XNB root reader index is out of bounds")); }
    result.root = Some(root);
    result.root_span = Some((root_start, offset));
    if shared != 0 { result.status = "SHARED_RESOURCES_NOT_PARSED"; return Ok(result); }
    if root == 0 {
        if offset != source.len() { return Err(invalid("unexpected bytes after a null XNB root")); }
        result.status = "NULL_ROOT";
        return Ok(result);
    }
    if !known_texture_reader(&result.readers[(root - 1) as usize]) {
        result.status = "CUSTOM_OR_UNSUPPORTED_ROOT_READER";
        return Ok(result);
    }
    result.texture_offset = Some(offset);
    let format = source.u32(&mut offset)?;
    let width = source.u32(&mut offset)?;
    let height = source.u32(&mut offset)?;
    let levels = source.u32(&mut offset)?;
    if width == 0 || height == 0 || width > 32768 || height > 32768
        || levels == 0 || levels > 32 - width.max(height).leading_zeros() {
        return Err(invalid("invalid XNB Texture2D dimensions/mip count"));
    }
    result.texture = Some((format, width, height, levels));
    let Some(layout) = texture_layout(format) else {
        result.status = "UNSUPPORTED_SURFACE_FORMAT";
        return Ok(result);
    };
    for level in 0..levels {
        let length_offset = offset;
        let size = source.u32(&mut offset)? as u64;
        let w = (width >> level).max(1);
        let h = (height >> level).max(1);
        if size != layout.bytes(w, h)? { return Err(invalid("XNB mip length differs from its storage shape")); }
        source.range(offset, size)?;
        result.mips.push(Mip { level, width: w, height: h, length_offset, offset, size });
        offset = offset.checked_add(size).ok_or_else(|| invalid("XNB mip offset overflow"))?;
    }
    if offset != source.len() { return Err(invalid("unexpected trailing XNB root/shared data")); }
    result.status = "METADATA_ONLY";
    Ok(result)
}

/// Development reader: uncompressed XNB v5 desktop root Texture2D metadata only.
/// Pixel blobs are range-checked and skipped, not read/decoded/rewritten. The old
/// generic container-audit/header output contract is intentionally unchanged.
pub fn texture_audit(path: &Path) -> io::Result<()> {
    use super::audit_io::{field, Source};
    let mut source = Source::open(path, u32::MAX as u64, TEXTURE_AUDIT_BUDGET)?;
    let v = content_inventory(&mut source)?;
    source.unchanged()?;
    println!("XNB_CONTENT_HEADER|{}|{}|{}|{}", v.header.target as char,
        v.header.version, v.header.flags, v.header.size);
    for (i, reader) in v.readers.iter().enumerate() {
        println!("XNB_READER|{}|{}|{}", i + 1, reader.version, field(reader.name.as_bytes()));
        println!("XNB_READER_SPAN|{}|{}|{}|{}|{}|ORIGINAL_FILE_OFFSETS", i + 1,
            reader.record_start, reader.name_offset, reader.name_size, reader.version_offset);
    }
    if let Some(table_end) = v.table_end { println!("XNB_READER_TABLE_SPAN|10|{table_end}|ORIGINAL_FILE_OFFSETS"); }
    if let Some((start, end)) = v.root_span { println!("XNB_ROOT_INDEX_SPAN|{start}|{end}|ORIGINAL_FILE_OFFSETS"); }
    if let Some(offset) = v.texture_offset { println!("XNB_TEXTURE_HEADER_SPAN|{offset}|16|ORIGINAL_FILE_OFFSETS"); }
    if let Some(shared) = v.shared { println!("XNB_SHARED|{shared}"); }
    if let Some(root) = v.root { println!("XNB_ROOT|{root}"); }
    if let Some((format, width, height, levels)) = v.texture {
        println!("XNB_TEXTURE|{format}|{width}|{height}|{levels}");
    }
    for mip in &v.mips {
        println!("XNB_MIP|{}|{}|{}|{}|{}", mip.level, mip.width, mip.height, mip.offset, mip.size);
        println!("XNB_MIP_SPAN|{}|{}|{}|{}|ORIGINAL_FILE_OFFSETS", mip.level, mip.length_offset, mip.offset, mip.size);
    }
    let kind = if v.status == "METADATA_ONLY" { "METADATA_ONLY" } else { "OPAQUE" };
    println!("XNB_TEXTURE_STATUS|{kind}|{}", v.status);
    eprintln!("Development read-only XNB metadata audit; pixel bytes, atlas ownership and runtime compatibility are not verified. A detached experimental exporter exists separately.");
    Ok(())
}

/// Development-only detached export for a single ordinary v5 Texture2D root.
/// The source, format and reader table are preserved; only one BC base image is
/// resized and re-encoded. No game file is replaced or marked compatible.
pub fn texture_export(max_edge: u32, input: &Path, output: &Path) -> io::Result<()> {
    use image::imageops::FilterType;
    use image_dds::{ImageFormat, Mipmaps, Quality, Surface, SurfaceRgba8};
    use super::audit_io::Source;
    if !(64..=8192).contains(&max_edge) { return Err(invalid("XNB export max edge must be 64..8192")); }
    if fs::symlink_metadata(output).is_ok() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "XNB export destination exists")); }
    const MAX_EXPORT: u64 = 64 * 1024 * 1024;
    let mut source = Source::open(input, MAX_EXPORT, MAX_EXPORT + TEXTURE_AUDIT_BUDGET)?;
    let inventory = content_inventory(&mut source)?;
    if inventory.status != "METADATA_ONLY" || inventory.mips.len() != 1 || inventory.shared != Some(0) {
        return Err(invalid("XNB export requires one ordinary uncompressed v5 Texture2D mip and no shared resources"));
    }
    let (surface_id, width, height, levels) = inventory.texture.ok_or_else(|| invalid("missing XNB texture header"))?;
    if levels != 1 || (width as u64) * (height as u64) > 16_777_216
        || super::texture_policy::small_or_thin(width, height)
        || super::texture_policy::atlas_hint(input) {
        return Err(invalid("XNB texture shape or atlas name is outside export policy"));
    }
    let format = match surface_id {
        4 => ImageFormat::BC1RgbaUnorm,
        5 => ImageFormat::BC2RgbaUnorm,
        6 => ImageFormat::BC3RgbaUnorm,
        _ => return Err(invalid("XNB export supports only BC1/BC2/BC3 surfaces")),
    };
    let edge = super::texture_policy::effective_edge(max_edge, width, height);
    let (new_width, new_height) = if width >= height {
        (width.min(edge), ((height as u64 * width.min(edge) as u64 + width as u64 / 2) / width as u64) as u32)
    } else {
        (((width as u64 * height.min(edge) as u64 + height as u64 / 2) / height as u64) as u32, height.min(edge))
    };
    if new_width == width && new_height == height {
        source.unchanged()?;
        println!("XNB_TEXTURE_EXPORT|{}|{}|{}|{}|NO_GAIN", source.len(), source.len(), width, height);
        return Ok(());
    }
    if super::texture_policy::output_too_thin(new_width, new_height) {
        return Err(invalid("XNB resized texture would be too thin"));
    }
    let bytes = source.read_at(0, source.len() as usize)?;
    source.unchanged()?;
    let mip = &inventory.mips[0];
    let data = bytes.get(mip.offset as usize..(mip.offset + mip.size) as usize)
        .ok_or_else(|| invalid("XNB source mip is out of bounds"))?;
    let texture_offset = inventory.texture_offset.ok_or_else(|| invalid("XNB texture offset missing"))? as usize;
    if rebuild_single_mip(&bytes, texture_offset, surface_id, width, height, data)? != bytes {
        return Err(invalid("XNB no-change rebuild differs from its source"));
    }
    let decoded = Surface { width, height, depth: 1, layers: 1, mipmaps: 1,
        image_format: format, data }.decode_rgba8()
        .map_err(|error| invalid(&format!("XNB source texture decode failed: {error}")))?;
    let base = decoded.get_image(0, 0, 0).ok_or_else(|| invalid("XNB base texture missing"))?;
    let resized = image::imageops::resize(&base, new_width, new_height, FilterType::Lanczos3);
    let encoded = SurfaceRgba8::from_image(&resized).encode(format, Quality::Normal, Mipmaps::Disabled)
        .map_err(|error| invalid(&format!("XNB texture encode failed: {error}")))?;
    if encoded.width != new_width || encoded.height != new_height || encoded.mipmaps != 1 {
        return Err(invalid("XNB encoder returned an unexpected image shape"));
    }
    let mut encoded_bytes = encoded.data.to_vec();
    if surface_id == 4 { encode_bc1_alpha_blocks(&mut encoded_bytes, &resized)?; }
    let encoded_bytes: &[u8] = &encoded_bytes;
    let decoded_candidate = Surface { width: new_width, height: new_height, depth: 1,
        layers: 1, mipmaps: 1, image_format: format, data: encoded_bytes }.decode_rgba8()
        .map_err(|error| invalid(&format!("XNB exported texture decode failed: {error}")))?;
    if decoded_candidate.get_image(0, 0, 0).is_none_or(|image| image.dimensions() != (new_width, new_height)) {
        return Err(invalid("XNB exported base image has wrong dimensions"));
    }
    if surface_id == 4 {
        let image = decoded_candidate.get_image(0, 0, 0).ok_or_else(|| invalid("XNB exported base image missing"))?;
        if image.pixels().zip(resized.pixels()).any(|(encoded, source)|
            (encoded.0[3] < 128) != (source.0[3] < 128)) {
            return Err(invalid("XNB BC1 alpha mask changed during encoding"));
        }
    }
    let candidate = rebuild_single_mip(&bytes, texture_offset, surface_id, new_width, new_height, encoded_bytes)?;
    if candidate.len() >= bytes.len() {
        println!("XNB_TEXTURE_EXPORT|{}|{}|{}|{}|NO_GAIN", bytes.len(), candidate.len(), new_width, new_height);
        return Ok(());
    }
    // Verify the complete new XNB structure and its re-encoded base image before
    // creating a destination. Exact byte identity is retained outside the root.
    let tmp = std::env::temp_dir().join(format!("bgc-xnb-verify-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut created = false;
    let verification = (|| -> io::Result<()> {
        let mut temporary = fs::OpenOptions::new().write(true).create_new(true)
            .custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
        created = true;
        temporary.write_all(&candidate)?;
        temporary.sync_all()?;
        drop(temporary);
        let mut check = Source::open(&tmp, MAX_EXPORT, TEXTURE_AUDIT_BUDGET)?;
        let parsed = content_inventory(&mut check)?;
        if parsed.status != "METADATA_ONLY" || parsed.texture != Some((surface_id, new_width, new_height, 1)) {
            return Err(invalid("XNB exported structure failed independent parse"));
        }
        check.unchanged()?;
        Ok(())
    })();
    if created { let _ = fs::remove_file(&tmp); }
    verification?;
    source.unchanged()?;
    let mut destination = fs::OpenOptions::new().write(true).create_new(true)
        .custom_flags(libc::O_NOFOLLOW).open(output)?;
    let write = (|| -> io::Result<()> { destination.write_all(&candidate)?; destination.sync_all() })();
    if let Err(error) = write { let _ = fs::remove_file(output); return Err(error); }
    println!("XNB_TEXTURE_EXPORT|{}|{}|{}|{}|EXPORTED", bytes.len(), candidate.len(), new_width, new_height);
    eprintln!("Experimental detached XNB export: source unchanged; logical savings only. Runtime compatibility and atlas references are unverified.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(target: u8, version: u8, flags: u8, size: u32) -> Vec<u8> {
        let mut bytes = b"XNB".to_vec();
        bytes.extend([target, version, flags]);
        bytes.extend(size.to_le_bytes());
        bytes
    }

    #[test]
    fn validates_bounded_header_fields_without_interpreting_payload() {
        let bytes = fixture(b'd', 5, 0, 10);
        assert_eq!(parse(&bytes, 10).unwrap(), Header { target: b'd', version: 5, flags: 0, size: 10 });
        assert!(plausible(&fixture(b'd', 5, 0x40, 100), 100));
        for (target, version, flags, size, actual) in [
            (b'?', 5, 0, 10, 10), (b'd', 3, 0, 10, 10), (b'd', 7, 0, 10, 10),
            (b'd', 5, 2, 10, 10), (b'd', 5, 0xc0, 10, 10), (b'd', 5, 0, 11, 10),
        ] {
            assert!(parse(&fixture(target, version, flags, size), actual).is_err());
        }
        for n in 0..10 { assert!(parse(&fixture(b'd', 5, 0, 10)[..n], 10).is_err()); }
    }

    fn texture_fixture(reader: &[u8]) -> Vec<u8> {
        assert!(reader.len() < 128);
        let mut bytes = fixture(b'w', 5, 0, 0);
        bytes.push(1); // one content reader
        bytes.push(reader.len() as u8);
        bytes.extend_from_slice(reader);
        bytes.extend_from_slice(&0u32.to_le_bytes()); // reader version
        bytes.push(0); // no shared resources
        bytes.push(1); // root uses the single reader
        for value in [4u32, 8, 8, 2] { bytes.extend_from_slice(&value.to_le_bytes()); }
        bytes.extend_from_slice(&32u32.to_le_bytes());
        bytes.extend([0x11; 32]); // BC1 8x8 base mip
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend([0x22; 8]); // BC1 4x4 mip
        let size = bytes.len() as u32;
        bytes[6..10].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    #[test]
    fn v5_texture_inventory_tracks_exact_mips_and_rejects_bad_lengths() {
        let path = std::env::temp_dir().join(format!("bgc-xnb-test-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let original = texture_fixture(b"Microsoft.Xna.Framework.Content.Texture2DReader, Microsoft.Xna.Framework");
        let run = |bytes: &[u8]| {
            fs::write(&path, bytes).unwrap();
            let mut source = super::super::audit_io::Source::open(&path, u32::MAX as u64, TEXTURE_AUDIT_BUDGET).unwrap();
            content_inventory(&mut source)
        };
        let result = (|| {
            let inventory = run(&original).unwrap();
            assert_eq!(inventory.status, "METADATA_ONLY");
            assert_eq!(inventory.texture, Some((4, 8, 8, 2)));
            assert_eq!(inventory.mips.iter().map(|m| (m.width, m.height, m.size)).collect::<Vec<_>>(),
                vec![(8, 8, 32), (4, 4, 8)]);
            let bare = texture_fixture(b"Microsoft.Xna.Framework.Content.Texture2DReader");
            assert_eq!(run(&bare).unwrap().status, "METADATA_ONLY");
            let custom = texture_fixture(b"Game.Content.Texture2DReader");
            assert_eq!(run(&custom).unwrap().status, "CUSTOM_OR_UNSUPPORTED_ROOT_READER");
            let mut bad = original.clone();
            let first_length = inventory.mips[0].length_offset as usize;
            bad[first_length..first_length + 4].copy_from_slice(&31u32.to_le_bytes());
            assert!(run(&bad).is_err());
            let mut trailing = original.clone();
            trailing.push(0);
            let length = trailing.len() as u32;
            trailing[6..10].copy_from_slice(&length.to_le_bytes());
            assert!(run(&trailing).is_err());
        })();
        let _ = fs::remove_file(&path);
        result
    }

    #[test]
    fn detached_bc1_export_shrinks_and_preserves_source() {
        let root = std::env::temp_dir().join(format!("bgc-xnb-export-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let input = root.join("page.xnb");
        let output = root.join("smaller.xnb");
        let reader = b"Microsoft.Xna.Framework.Content.Texture2DReader";
        let mut bytes = texture_fixture(reader);
        let texture_start = 10 + 1 + 1 + reader.len() + 4 + 1 + 1;
        bytes.truncate(texture_start);
        for value in [4u32, 1024, 1024, 1, 524288] { bytes.extend_from_slice(&value.to_le_bytes()); }
        bytes.extend(vec![0x11; 524288]);
        let length = bytes.len() as u32;
        bytes[6..10].copy_from_slice(&length.to_le_bytes());
        fs::write(&input, &bytes).unwrap();
        let no_gain = root.join("no-gain.xnb");
        texture_export(2048, &input, &no_gain).unwrap();
        assert!(!no_gain.exists());
        texture_export(512, &input, &output).unwrap();
        assert_eq!(fs::read(&input).unwrap(), bytes);
        assert!(fs::metadata(&output).unwrap().len() < bytes.len() as u64);
        let mut check = crate::audit_io::Source::open(&output, u32::MAX as u64, TEXTURE_AUDIT_BUDGET).unwrap();
        let parsed = content_inventory(&mut check).unwrap();
        assert_eq!(parsed.texture, Some((4, 512, 512, 1)));
        assert_eq!(parsed.status, "METADATA_ONLY");
        assert!(texture_export(512, &input, &output).is_err());
        let mut transparent = bytes.clone();
        let payload = texture_start + 20;
        transparent[payload..payload + 8].copy_from_slice(&[0, 0, 0, 0, 255, 255, 255, 255]);
        fs::write(&input, transparent).unwrap();
        let preserved = root.join("transparent.xnb");
        texture_export(512, &input, &preserved).unwrap();
        let bytes = fs::read(&preserved).unwrap();
        let mut check = crate::audit_io::Source::open(&preserved, u32::MAX as u64, TEXTURE_AUDIT_BUDGET).unwrap();
        let parsed = content_inventory(&mut check).unwrap();
        let mip = &parsed.mips[0];
        let pixels = image_dds::Surface { width: 512, height: 512, depth: 1, layers: 1, mipmaps: 1,
            image_format: image_dds::ImageFormat::BC1RgbaUnorm,
            data: &bytes[mip.offset as usize..(mip.offset + mip.size) as usize] }.decode_rgba8().unwrap();
        assert!(pixels.get_image(0, 0, 0).unwrap().pixels().any(|pixel| pixel.0[3] == 0));
        fs::remove_dir_all(root).unwrap();
    }
}
