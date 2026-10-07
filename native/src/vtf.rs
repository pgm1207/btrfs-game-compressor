//! Development-only PC VTF 7.1–7.4 metadata audit. No pixel decoder, VPK reader
//! or writer. Field layouts/storage order are documented in Valve's SDK vtf.h;
//! do not cast C++ structs or reuse DDS's format IDs/mip ordering.
use std::{collections::BTreeSet, io, path::Path};
use super::{audit_io::{le16, le32, Layout, Source}, invalid};

const MAX_READ: u64 = 64 * 1024;
const MAX_RESOURCES: u32 = 32;
const LOW_IMAGE: u32 = 0x01;
const HIGH_IMAGE: u32 = 0x30;
const ENVMAP: u32 = 0x4000;

struct Header {
    minor: u32, size: u32, width: u32, height: u32, flags: u32,
    frames: u16, first_frame: u16, format: u32, mips: u32,
    low_format: u32, low_width: u32, low_height: u32, depth: u32,
}
struct Resource { id: u32, flags: u8, value: u32 }
struct Mip { level: u32, width: u32, height: u32, offset: u64, size: u64 }
struct Inventory {
    header: Header, resources: Vec<Resource>, mips: Vec<Mip>,
    high_bytes: u64, low_bytes: u64, status: &'static str,
}

fn image_layout(format: u32) -> Option<Layout> {
    // Only stable common PC enum values. These are storage shapes, not an
    // endorsement to resize normal/data textures or reinterpret channel order.
    Some(match format {
        0 | 1 | 11 | 12 | 16 => Layout::pixels(4),
        2 | 3 => Layout::pixels(3),
        4 | 6 | 17 | 18 | 19 | 21 => Layout::pixels(2),
        5 | 8 => Layout::pixels(1),
        13 | 20 => Layout::bc(8),
        14 | 15 => Layout::bc(16),
        _ => return None,
    })
}

fn image_name(format: u32) -> &'static str {
    match format {
        0 => "RGBA8888", 1 => "ABGR8888", 2 => "RGB888", 3 => "BGR888",
        4 => "RGB565", 5 => "I8", 6 => "IA88", 8 => "A8", 11 => "ARGB8888",
        12 => "BGRA8888", 13 => "DXT1", 14 => "DXT3", 15 => "DXT5",
        16 => "BGRX8888", 17 => "BGR565", 18 => "BGRX5551", 19 => "BGRA4444",
        20 => "DXT1_ONEBITALPHA", 21 => "BGRA5551", u32::MAX => "NONE",
        _ => "UNKNOWN",
    }
}

fn inspect(source: &mut Source) -> io::Result<Inventory> {
    let base = source.read_at(0, 16)?;
    if &base[..4] != b"VTF\0" || le32(&base, 4)? != 7 {
        return Err(invalid("not a supported PC VTF header"));
    }
    let minor = le32(&base, 8)?;
    let size = le32(&base, 12)?;
    if !(1..=4).contains(&minor) {
        return Err(invalid("VTF audit only understands PC versions 7.1–7.4"));
    }
    // Conservative layouts. Old 7.1 headers can be 64 or 80 bytes; no native
    // struct sizeof/alignment is used to infer the on-disk resource directory.
    if minor == 1 && !matches!(size, 64 | 80)
        || minor == 2 && size != 80
        || minor >= 3 && !(80..=80 + MAX_RESOURCES * 8).contains(&size) {
        return Err(invalid("unsupported VTF header size"));
    }
    let bytes = source.read_at(0, size as usize)?;
    let header = Header {
        minor, size, width: le16(&bytes, 16)? as u32, height: le16(&bytes, 18)? as u32,
        flags: le32(&bytes, 20)?, frames: le16(&bytes, 24)?, first_frame: le16(&bytes, 26)?,
        format: le32(&bytes, 52)?, mips: bytes[56] as u32,
        low_format: le32(&bytes, 57)?, low_width: bytes[61] as u32, low_height: bytes[62] as u32,
        depth: if minor >= 2 { le16(&bytes, 63)? as u32 } else { 1 },
    };
    if header.width == 0 || header.height == 0 || header.width > 32768 || header.height > 32768
        || header.depth == 0 || header.frames == 0 || header.mips == 0
        || header.mips > 32 - header.width.max(header.height).max(header.depth).leading_zeros()
        || (header.low_width == 0) != (header.low_height == 0) {
        return Err(invalid("invalid VTF dimensions/frame/mip fields"));
    }
    let mut resources = Vec::new();
    if minor >= 3 {
        let count = le32(&bytes, 68)?;
        if count > MAX_RESOURCES || size != 80 + count * 8 {
            return Err(invalid("inconsistent VTF resource directory size"));
        }
        let mut ids = BTreeSet::new();
        for i in 0..count as usize {
            let pos = 80 + i * 8;
            let tag = le32(&bytes, pos)?;
            let id = tag & 0x00ffffff;
            if !ids.insert(id) { return Err(invalid("duplicate VTF resource identifier")); }
            resources.push(Resource { id, flags: (tag >> 24) as u8, value: le32(&bytes, pos + 4)? });
        }
    }
    let mut v = Inventory { header, resources, mips: Vec::new(),
        high_bytes: 0, low_bytes: 0, status: "METADATA_ONLY" };
    // Multisurface/cube face-count rules and volume storage are separate work.
    if v.header.depth != 1 || v.header.frames != 1 || v.header.flags & ENVMAP != 0 {
        v.status = "MULTISURFACE_STORAGE_NOT_PARSED";
        return Ok(v);
    }
    // Source streaming/procedural layouts require more than a contiguous chain.
    if v.header.flags & (0xc0000000 | 0x800) != 0 {
        v.status = "STREAMED_OR_PROCEDURAL_STORAGE_NOT_PARSED";
        return Ok(v);
    }
    let Some(high_layout) = image_layout(v.header.format) else {
        v.status = "UNSUPPORTED_IMAGE_FORMAT";
        return Ok(v);
    };
    if v.header.low_width != 0 {
        let Some(low_layout) = image_layout(v.header.low_format) else {
            v.status = "UNSUPPORTED_THUMBNAIL_FORMAT";
            return Ok(v);
        };
        v.low_bytes = low_layout.bytes(v.header.low_width, v.header.low_height)?;
    }
    for level in 0..v.header.mips {
        v.high_bytes = v.high_bytes.checked_add(high_layout.bytes(
            (v.header.width >> level).max(1), (v.header.height >> level).max(1))?)
            .ok_or_else(|| invalid("VTF mip chain length overflow"))?;
    }
    let mut extents = Vec::new();
    let high_start;
    if minor < 3 {
        high_start = (size as u64).checked_add(v.low_bytes).ok_or_else(|| invalid("VTF image offset overflow"))?;
        source.range(size as u64, v.low_bytes)?;
        source.range(high_start, v.high_bytes)?;
        if high_start.checked_add(v.high_bytes) != Some(source.len()) {
            v.status = "UNKNOWN_TRAILING_LAYOUT";
            return Ok(v);
        }
    } else {
        let mut high = None;
        let mut low = None;
        for resource in &v.resources {
            super::cancelled()?;
            if resource.flags & !2 != 0 {
                v.status = "UNKNOWN_RESOURCE_FLAGS";
                return Ok(v);
            }
            if resource.flags & 2 != 0 {
                if matches!(resource.id, LOW_IMAGE | HIGH_IMAGE) {
                    return Err(invalid("VTF image resource cannot contain an inline value"));
                }
                continue;
            }
            let start = resource.value as u64;
            if start < size as u64 { return Err(invalid("VTF resource overlaps its header")); }
            let length = match resource.id {
                HIGH_IMAGE => { high = Some(start); v.high_bytes }
                LOW_IMAGE => { low = Some(start); v.low_bytes }
                _ => {
                    let prefix = source.read_at(start, 4)?;
                    (le32(&prefix, 0)? as u64).checked_add(4)
                        .ok_or_else(|| invalid("VTF resource length overflow"))?
                }
            };
            source.range(start, length)?;
            if length != 0 { extents.push((start, start + length)); }
        }
        if v.low_bytes != 0 && low.is_none() { return Err(invalid("VTF thumbnail lacks its resource entry")); }
        high_start = high.ok_or_else(|| invalid("VTF lacks its high-resolution image resource"))?;
        extents.sort_unstable();
        if extents.windows(2).any(|p| p[0].1 > p[1].0) {
            return Err(invalid("overlapping VTF resource extents"));
        }
        // Unknown resources have only length/bounds inspected. Gaps and trailers
        // are retained/unknown, never called removable space or eligible savings.
    }
    let mut offset = high_start;
    // VTF disk order is smallest stored mip to largest (opposite DDS).
    for level in (0..v.header.mips).rev() {
        let width = (v.header.width >> level).max(1);
        let height = (v.header.height >> level).max(1);
        let length = high_layout.bytes(width, height)?;
        source.range(offset, length)?;
        v.mips.push(Mip { level, width, height, offset, size: length });
        offset = offset.checked_add(length).ok_or_else(|| invalid("VTF mip offset overflow"))?;
    }
    Ok(v)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, 4 * 1024 * 1024 * 1024, MAX_READ)?;
    let v = inspect(&mut source)?;
    source.unchanged()?;
    let h = &v.header;
    println!("VTF_HEADER|7|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}", h.minor, h.size,
        h.width, h.height, h.depth, h.frames, h.first_frame, h.flags,
        h.format, image_name(h.format), h.mips);
    println!("VTF_THUMBNAIL|{}|{}|{}|{}", h.low_format, image_name(h.low_format), h.low_width, h.low_height);
    for resource in &v.resources {
        println!("VTF_RESOURCE|{:06X}|{}|{}|{}", resource.id, resource.flags, resource.value,
            if resource.flags & 2 != 0 { "INLINE_VALUE" } else { "DECLARED_OFFSET" });
    }
    for mip in &v.mips {
        println!("VTF_MIP|{}|{}|{}|{}|{}", mip.level, mip.width, mip.height, mip.offset, mip.size);
    }
    println!("VTF_RISK|{}|{}|{}", u8::from(h.flags & 0x80 != 0),
        u8::from(h.flags & 0x08000000 != 0),
        u8::from(v.resources.iter().any(|r| r.id == 0x10)));
    println!("VTF_STATUS|{}|{}", if v.status == "METADATA_ONLY" { "METADATA_ONLY" } else { "OPAQUE" }, v.status);
    eprintln!("Development read-only VTF metadata audit; pixel bytes, material/atlas ownership, checksums and runtime compatibility are not verified. No VTF/VPK writer exists.");
    Ok(())
}
