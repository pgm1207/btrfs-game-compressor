//! Development-only, read-only cooked Unreal Engine 4 Texture2D audit and a
//! detached inline-DDS exporter for the same narrow, empirically verified
//! window as `unreal_package`: LegacyFileVersion -6/-7, UE4 file version
//! 459-484, licensee 0, empty custom-version container, `PKG_FilterEditorOnly`.
//!
//! Verified class-data layout for that window (byte-checked against six real
//! `Calm Down, Stalin` Texture2D packages and cross-read against UEViewer's
//! `UTexture2D::Serialize4`/`FTexturePlatformData` documentation and the public
//! UE4 `UE4Version.h` constants; no runtime dependency):
//!
//! 1. cooked `FPropertyTag` stream (values inline) terminated by `FName "None"`;
//! 2. a 12-byte prefix: 32-bit zero word, two `FStripDataFlags` byte pairs and a
//!    `bCooked` 32-bit word. The leading zero word is unexplained by the public
//!    serializers and is required to be zero; the exact split between the two
//!    strip-flag pairs is inferred from their values (editor data stripped);
//! 3. a format list: `FName` pixel-format name, 32-bit `SkipOffset`, then one
//!    `FTexturePlatformData`; `FName "None"` ends the list;
//! 4. `FTexturePlatformData`: `SizeX`, `SizeY`, `NumSlices`, FString
//!    pixel format, `FirstMipToSerialize`, `NumMips`;
//! 5. per mip: 32-bit cooked word, `FByteBulkData` (flags, element count, size
//!    on disk, 64-bit offset), inline payload when not stored elsewhere, then
//!    `SizeX` and `SizeY` (no `SizeZ` before UE4.20).
//!
//! The declared `SkipOffset` must equal the position after the platform data,
//! and the terminating `FName "None"` must land exactly at the export end. Mip
//! payloads are only range-checked here, never decoded; the format list is
//! parsed but no dictionary or side-car file is consulted.
use std::{io, path::Path};

use super::{
    audit_io::Source,
    invalid,
    unreal_package::{self, Export, Report},
};

const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;
const READ_BUDGET: u64 = 256 * 1024 * 1024;
const EXPORT_LIMIT: u64 = 256 * 1024 * 1024;
const MAX_TEXTURES: usize = 32;
const MAX_FORMATS: usize = 8;
const MAX_MIPS: i32 = 32;
const MAX_STRING: usize = 256;
const MAX_DIMENSION: i32 = 16384;
const MAX_PIXELS: u64 = 268_435_456;
const WINDOW: usize = 64 * 1024;
const READ_CHUNK: usize = 1024 * 1024;

// UE4 `EBulkDataFlags` values understood by this reader.
const BULK_PAYLOAD_AT_END: u32 = 0x1;
const BULK_COMPRESSED_ZLIB: u32 = 0x2;
const BULK_SINGLE_USE: u32 = 0x8;
const BULK_FORCE_INLINE: u32 = 0x40;
const BULK_SEPARATE_FILE: u32 = 0x100;
const BULK_OPTIONAL: u32 = 0x800;
const BULK_SIZE_64_BIT: u32 = 0x2000;
const BULK_KNOWN: u32 = BULK_PAYLOAD_AT_END
    | BULK_COMPRESSED_ZLIB
    | BULK_SINGLE_USE
    | BULK_FORCE_INLINE
    | BULK_SEPARATE_FILE
    | BULK_OPTIONAL
    | BULK_SIZE_64_BIT;

fn unsupported_text(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message.into())
}

/// Sequential little-endian reader with a bounded refill window and an explicit
/// extent limit, mirroring the `unreal_package` cursor conventions: every read
/// is checked against both the opened source and the export being inspected.
struct Cursor<'a> {
    source: &'a mut Source,
    length: u64,
    limit: u64,
    position: u64,
    start: u64,
    window: Vec<u8>,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a mut Source) -> Self {
        let length = source.len();
        Self { source, length, limit: length, position: 0, start: 0, window: Vec::new() }
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn len(&self) -> u64 {
        self.length
    }

    /// Restore the readable extent to the whole source between exports.
    fn clear_limit(&mut self) {
        self.limit = self.length;
    }

    /// Shrink the readable extent to one export. Payload skips and header reads
    /// both stay inside the selected export.
    fn set_limit(&mut self, limit: u64) -> io::Result<()> {
        if limit > self.length || limit < self.position {
            return Err(invalid("Unreal texture extent is outside its source"));
        }
        self.limit = limit;
        Ok(())
    }

    fn seek(&mut self, position: u64) -> io::Result<()> {
        if position > self.limit { return Err(invalid("Unreal texture offset beyond its extent")); }
        self.position = position;
        Ok(())
    }

    fn ensure(&mut self, count: usize) -> io::Result<()> {
        let end = self.position.checked_add(count as u64)
            .ok_or_else(|| invalid("Unreal texture offset overflow"))?;
        if end > self.limit { return Err(invalid("truncated Unreal texture record")); }
        if self.position >= self.start
            && self.start.checked_add(self.window.len() as u64).is_some_and(|window| end <= window) {
            return Ok(());
        }
        let want = (count.max(WINDOW) as u64).min(self.limit - self.position) as usize;
        if want < count { return Err(invalid("truncated Unreal texture record")); }
        self.window = self.source.read_at(self.position, want)?;
        self.start = self.position;
        Ok(())
    }

    fn take(&mut self, count: usize) -> io::Result<&[u8]> {
        self.ensure(count)?;
        let begin = usize::try_from(self.position - self.start)
            .map_err(|_| invalid("Unreal texture offset overflow"))?;
        let end = begin.checked_add(count).ok_or_else(|| invalid("Unreal texture offset overflow"))?;
        self.position += count as u64;
        self.window.get(begin..end).ok_or_else(|| invalid("truncated Unreal texture record"))
    }

    fn skip(&mut self, count: u64) -> io::Result<()> {
        let end = self.position.checked_add(count)
            .ok_or_else(|| invalid("Unreal texture offset overflow"))?;
        self.seek(end)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// An i32-prefixed `FString` bounded to `max` bytes of text. Positive lengths
    /// are UTF-8 with a trailing NUL, negative lengths UTF-16; control characters
    /// and pipes are refused so records stay single-line and unambiguous.
    fn string(&mut self, max: usize) -> io::Result<String> {
        let size = self.i32()?;
        let count = size.checked_abs().ok_or_else(|| invalid("invalid Unreal string size"))? as usize;
        if count == 0 || count - 1 > max {
            return Err(invalid("Unreal texture string exceeds bounds"));
        }
        let text = if size > 0 {
            let bytes = self.take(count)?;
            if bytes[count - 1] != 0 { return Err(invalid("Unreal texture string is not terminated")); }
            std::str::from_utf8(&bytes[..count - 1])
                .map_err(|_| invalid("invalid Unreal texture UTF-8"))?.to_owned()
        } else {
            let bytes = self.take(count.checked_mul(2).ok_or_else(|| invalid("Unreal texture string overflows"))?)?;
            let values: Vec<u16> = bytes.chunks_exact(2)
                .map(|pair| u16::from_le_bytes(pair.try_into().unwrap())).collect();
            if values[count - 1] != 0 { return Err(invalid("Unreal UTF-16 string is not terminated")); }
            String::from_utf16(&values[..count - 1]).map_err(|_| invalid("invalid Unreal UTF-16"))?
        };
        if text.chars().any(|c| c.is_control() || c == '|') { return Err(invalid("unsafe Unreal texture text")); }
        Ok(text)
    }

    /// Append `count` payload bytes without staging them in a window. The read
    /// budget is enforced by `Source`; cancellation is checked per chunk.
    fn read_into(&mut self, out: &mut Vec<u8>, count: u64) -> io::Result<()> {
        let end = self.position.checked_add(count).ok_or_else(|| invalid("Unreal texture offset overflow"))?;
        if end > self.limit { return Err(invalid("truncated Unreal texture payload")); }
        out.try_reserve(count as usize)
            .map_err(|_| invalid("Unreal texture allocation limit exceeded"))?;
        let mut offset = self.position;
        while offset < end {
            super::cancelled()?;
            let amount = (end - offset).min(READ_CHUNK as u64) as usize;
            let bytes = self.source.read_at(offset, amount)?;
            if bytes.len() != amount { return Err(invalid("truncated Unreal texture payload")); }
            out.extend_from_slice(&bytes);
            offset += amount as u64;
        }
        self.position = end;
        self.window.clear();
        self.start = self.position;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Storage {
    Inline,
    Bulk,
}

impl Storage {
    fn word(self) -> &'static str {
        match self { Storage::Inline => "inline", Storage::Bulk => "bulk" }
    }
}

#[derive(Debug)]
struct Mip {
    size_on_disk: i64,
    storage: Storage,
    /// Absolute payload extent when the declared offset can be resolved: inline
    /// payloads use their position in the export, bulk payloads the declared
    /// package bulk-data base. `None` keeps the mip reported but unresolved.
    payload: Option<(u64, u64)>,
    width: i32,
    height: i32,
}

#[derive(Debug)]
struct PlatformEntry {
    format: String,
    width: i32,
    height: i32,
    slices: i32,
    first_mip: i32,
    mips: Vec<Mip>,
}

#[derive(Debug)]
struct Texture {
    object: String,
    entries: Vec<PlatformEntry>,
}

/// Verified byte layout `(block width, block height, bytes per block)` for the
/// cooked pixel formats this audit can size. A pixel format is a block of one.
/// An unknown format is reported but never shape-checked.
fn layout_of(format: &str) -> Option<(u32, u32, u64)> {
    Some(match format {
        "PF_DXT1" | "PF_BC4" => (4, 4, 8),
        "PF_DXT3" | "PF_DXT5" | "PF_BC5" | "PF_BC6H" | "PF_BC7" => (4, 4, 16),
        "PF_B8G8R8A8" | "PF_R8G8B8A8" => (1, 1, 4),
        "PF_G8" => (1, 1, 1),
        "PF_G16" | "PF_R16F" => (1, 1, 2),
        "PF_R32F" => (1, 1, 4),
        "PF_A16B16G16R16" | "PF_FloatRGBA" => (1, 1, 8),
        "PF_A32B32G32R32F" => (1, 1, 16),
        _ => return None,
    })
}

fn mip_bytes(layout: (u32, u32, u64), width: i32, height: i32) -> io::Result<u64> {
    if width <= 0 || height <= 0 { return Err(invalid("invalid Unreal texture mip extent")); }
    let blocks = (width as u64).div_ceil(layout.0 as u64)
        .checked_mul((height as u64).div_ceil(layout.1 as u64))
        .ok_or_else(|| invalid("Unreal texture mip size overflow"))?;
    blocks.checked_mul(layout.2).ok_or_else(|| invalid("Unreal texture mip size overflow"))
}

/// The minimum extent a mip of this layout is clamped to by the cooker: one
/// block for block-compressed formats, one pixel otherwise.
fn mip_floor(layout: (u32, u32, u64)) -> i32 {
    layout.0.max(layout.1) as i32
}

fn name_at(report: &Report, index: i32, number: i32) -> io::Result<String> {
    if number != 0 { return Err(invalid("Unreal name number is outside the verified window")); }
    let text = usize::try_from(index).ok().and_then(|index| report.names.get(index))
        .cloned().ok_or_else(|| invalid("Unreal name index is out of range"))?;
    if text.chars().count() > MAX_STRING {
        return Err(invalid("Unreal name exceeds the texture audit bound"));
    }
    Ok(text)
}

/// Skip the cooked property tag stream up to and including its `FName "None"`
/// terminator. Only tag layouts verified for this window are accepted, so an
/// unknown tag type refuses the texture instead of guessing its extent.
fn skip_property_stream(cursor: &mut Cursor, report: &Report) -> io::Result<()> {
    loop {
        let name = name_at(report, cursor.i32()?, cursor.i32()?)?;
        if name == "None" { return Ok(()); }
        let type_name = name_at(report, cursor.i32()?, cursor.i32()?)?;
        let size = cursor.i32()?;
        if size < 0 { return Err(invalid("Unreal property size is negative")); }
        cursor.i32()?; // ArrayIndex
        match type_name.as_str() {
            // Struct tags carry the struct name and, since UE4 441 (inside this
            // window), a 16-byte struct GUID before the value.
            "StructProperty" => {
                name_at(report, cursor.i32()?, cursor.i32()?)?;
                cursor.skip(16)?;
            }
            // Bools store their value in the tag; the value size must be zero.
            "BoolProperty" => {
                if size != 0 { return Err(invalid("Unreal bool property has a non-zero size")); }
                if cursor.u8()? > 1 { return Err(invalid("Unsupported Unreal bool property value")); }
                continue;
            }
            // Enum-tagged bytes carry the enum name before the value.
            "ByteProperty" | "EnumProperty" => { name_at(report, cursor.i32()?, cursor.i32()?)?; }
            "ArrayProperty" | "NameProperty" | "IntProperty" | "FloatProperty"
            | "DoubleProperty" | "StrProperty" | "TextProperty" | "ObjectProperty" => (),
            other => return Err(unsupported_text(format!("unsupported Unreal property tag type {other}"))),
        }
        cursor.skip(size as u64)?;
    }
}

fn parse_mip(
    cursor: &mut Cursor,
    format: &str,
    index: i32,
    bulk_base: Option<u64>,
    export_end: u64,
) -> io::Result<Mip> {
    let cooked = cursor.u32()?;
    if cooked > 1 { return Err(invalid("Unsupported Unreal mip cooked flag encoding")); }
    let flags = cursor.u32()?;
    if flags & !BULK_KNOWN != 0 {
        return Err(unsupported_text(format!("unsupported Unreal bulk-data flags 0x{flags:08X}")));
    }
    if flags & BULK_COMPRESSED_ZLIB != 0 {
        return Err(unsupported_text("compressed Unreal mip payloads are unsupported"));
    }
    if flags & BULK_SIZE_64_BIT != 0 {
        return Err(unsupported_text("64-bit Unreal bulk sizes are unsupported"));
    }
    if flags & (BULK_SEPARATE_FILE | BULK_OPTIONAL) != 0 {
        return Err(unsupported_text("Unreal mip payload is stored outside this file"));
    }
    let element_count = i64::from(cursor.i32()?);
    let size_on_disk = i64::from(cursor.i32()?);
    let declared_offset = cursor.i64()?;
    if element_count < 0 || size_on_disk < 0 || declared_offset < 0 {
        return Err(invalid("negative Unreal mip bulk field"));
    }
    // Byte bulk data: one byte per element, so both sizes must agree.
    if element_count != size_on_disk {
        return Err(invalid("Unreal mip element count disagrees with its size on disk"));
    }
    let (storage, payload) = if flags & BULK_PAYLOAD_AT_END != 0 {
        let payload = bulk_base
            .and_then(|base| base.checked_add(declared_offset as u64))
            .and_then(|start| start.checked_add(size_on_disk as u64).map(|end| (start, end)))
            .map(|(start, end)| {
                if end > cursor.len() { None } else { Some((start, end - start)) }
            })
            .flatten();
        if bulk_base.is_some() && payload.is_none() {
            return Err(invalid("Unreal mip bulk extent is outside the file"));
        }
        (Storage::Bulk, payload)
    } else {
        let start = cursor.position();
        if declared_offset as u64 != start {
            return Err(invalid("inline Unreal mip offset does not point at its payload"));
        }
        let end = start.checked_add(size_on_disk as u64)
            .ok_or_else(|| invalid("Unreal texture mip size overflow"))?;
        if end > export_end { return Err(invalid("inline Unreal mip payload exceeds the export extent")); }
        cursor.seek(end)?;
        (Storage::Inline, Some((start, size_on_disk as u64)))
    };
    let width = cursor.i32()?;
    let height = cursor.i32()?;
    if width <= 0 || height <= 0 { return Err(invalid("invalid Unreal texture mip extent")); }
    if let Some(layout) = layout_of(format) {
        let expected = mip_bytes(layout, width, height)?;
        if element_count as u64 != expected {
            return Err(invalid(&format!(
                "mip {index} declares {element_count} bytes but {format} at {width}x{height} needs {expected}")));
        }
    }
    Ok(Mip { size_on_disk, storage, payload, width, height })
}

fn parse_entry(
    cursor: &mut Cursor,
    format_name: &str,
    skip_offset: i64,
    bulk_base: Option<u64>,
    export_end: u64,
) -> io::Result<PlatformEntry> {
    let width = cursor.i32()?;
    let height = cursor.i32()?;
    let slices = cursor.i32()?;
    if width <= 0 || height <= 0 || slices <= 0 {
        return Err(invalid("invalid Unreal texture platform extent"));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(unsupported_text("Unreal texture platform dimensions exceed the audit limit"));
    }
    let format = cursor.string(MAX_STRING)?;
    if format != format_name {
        return Err(invalid("Unreal texture format name disagrees with its format string"));
    }
    let first_mip = cursor.i32()?;
    let mip_count = cursor.i32()?;
    if !(1..=MAX_MIPS).contains(&mip_count) {
        return Err(unsupported_text(format!("Unreal texture mip count {mip_count} is outside 1-{MAX_MIPS}")));
    }
    let layout = layout_of(&format);
    let mut mips: Vec<Mip> = Vec::new();
    for index in 0..mip_count {
        let mip = parse_mip(cursor, &format, index, bulk_base, export_end)?;
        if let Some(layout) = layout {
            if index == 0 && first_mip == 0 && (mip.width != width || mip.height != height) {
                return Err(invalid("Unreal texture base mip disagrees with its platform extent"));
            }
            if let Some(previous) = mips.last() {
                let floor = mip_floor(layout);
                let expected_width = (previous.width / 2).max(floor);
                let expected_height = (previous.height / 2).max(floor);
                if mip.width != expected_width || mip.height != expected_height {
                    return Err(invalid(&format!(
                        "mip {index} extent {}x{} does not halve from {}x{}",
                        mip.width, mip.height, previous.width, previous.height)));
                }
            }
        }
        mips.push(mip);
    }
    let data_end = cursor.position();
    if skip_offset < 0 || data_end != skip_offset as u64 {
        return Err(invalid("Unreal texture platform data disagrees with its skip offset"));
    }
    Ok(PlatformEntry { format, width, height, slices, first_mip, mips })
}

/// Parse one Texture2D export's serialized data. The declared `SkipOffset` after
/// each platform entry must match, the format list must terminate exactly at the
/// export end, and every referenced extent must stay inside the file.
fn parse_texture(
    cursor: &mut Cursor,
    report: &Report,
    export: &Export,
    bulk_base: Option<u64>,
) -> io::Result<Texture> {
    let object = name_at(report, export.object_name as i32, 0)?;
    if export.serial_size <= 0 {
        return Err(invalid("Texture2D export has no serialized data"));
    }
    let start = export.serial_offset as u64;
    let end = start.checked_add(export.serial_size as u64)
        .ok_or_else(|| invalid("Unreal texture export extent overflow"))?;
    cursor.clear_limit();
    cursor.seek(start)?;
    cursor.set_limit(end)?;
    skip_property_stream(cursor, report)?;
    let leading = cursor.u32()?;
    if leading != 0 { return Err(unsupported_text("unknown Unreal texture serialization prefix")); }
    let global = cursor.u8()?;
    let class = cursor.u8()?;
    let global2 = cursor.u8()?;
    let class2 = cursor.u8()?;
    if global & 1 == 0 || global2 & 1 == 0 {
        return Err(unsupported_text("Texture2D editor data is not stripped"));
    }
    if class != 0 || class2 != 0 {
        return Err(unsupported_text("Texture2D class strip flags are unsupported"));
    }
    let cooked = cursor.u32()?;
    if cooked != 1 { return Err(invalid("Texture2D export is not cooked")); }
    let mut entries = Vec::new();
    loop {
        let format_name = name_at(report, cursor.i32()?, cursor.i32()?)?;
        if format_name == "None" { break; }
        if entries.len() >= MAX_FORMATS {
            return Err(unsupported_text("Unreal texture format list exceeds the audit limit"));
        }
        let skip_offset = i64::from(cursor.i32()?);
        if skip_offset < 0 || skip_offset as u64 > end {
            return Err(invalid("Unreal texture skip offset is outside the export"));
        }
        entries.push(parse_entry(cursor, &format_name, skip_offset, bulk_base, end)?);
    }
    if cursor.position() != end {
        return Err(invalid("Unreal texture format list does not end at the export extent"));
    }
    if entries.is_empty() {
        return Err(invalid("Texture2D export has no platform entry"));
    }
    Ok(Texture { object, entries })
}

fn render(lines: &mut Vec<String>, texture: &Texture, export: &Export) {
    let Some(entry) = texture.entries.first() else { return; };
    let storage = if entry.mips.iter().all(|mip| mip.storage == Storage::Inline) {
        "inline"
    } else if entry.mips.iter().all(|mip| mip.storage == Storage::Bulk) {
        "bulk"
    } else {
        "mixed"
    };
    let mip0 = entry.mips.first().map_or(0, |mip| mip.size_on_disk);
    lines.push(format!("UNREAL_TEXTURE|{}|{}|{}|{}|{}|{}|{}", texture.object,
        entry.width, entry.height, entry.format, entry.mips.len(), storage, mip0));
    if texture.entries.len() > 1 {
        lines.push(format!("UNREAL_TEXTURE_PLATFORMS|{}|{}", texture.object, texture.entries.len()));
    }
    let layout = layout_of(&entry.format);
    let mut declared = 0i64;
    let mut expected_total = 0u64;
    for (index, mip) in entry.mips.iter().enumerate() {
        declared = declared.saturating_add(mip.size_on_disk);
        let expected = layout
            .and_then(|layout| mip_bytes(layout, mip.width, mip.height).ok())
            .map_or_else(|| "unknown".to_string(), |bytes| bytes.to_string());
        if let Some(bytes) = layout.and_then(|layout| mip_bytes(layout, mip.width, mip.height).ok()) {
            expected_total = expected_total.saturating_add(bytes);
        }
        let start = mip.payload
            .map_or_else(|| "unresolved".to_string(), |(start, _)| start.to_string());
        lines.push(format!("UNREAL_TEXTURE_MIP|{}|0|{}|{}|{}|{}|{}|{}|{}", texture.object, index,
            mip.width, mip.height, mip.size_on_disk, mip.storage.word(), expected, start));
    }
    let expected_word = if layout.is_some() { expected_total.to_string() } else { "unknown".to_string() };
    lines.push(format!("UNREAL_TEXTURE_TOTAL|{}|{}|{}|{}", texture.object, declared,
        expected_word, export.serial_size));
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, MAX_FILE, READ_BUDGET)?;
    let report = unreal_package::inspect(&mut source)?;
    let bulk_base = unreal_package::bulk_data_start(&mut source, &report)?;
    let mut lines = Vec::new();
    let mut total = 0usize;
    let mut processed = 0usize;
    let mut parsed = 0usize;
    let mut refused = 0usize;
    {
        let mut cursor = Cursor::new(&mut source);
        for export in &report.exports {
            if report.class_name(export.class_index)? != "Texture2D" { continue; }
            total += 1;
            if total > MAX_TEXTURES { continue; }
            processed += 1;
            let object = name_at(&report, export.object_name as i32, 0)?;
            match parse_texture(&mut cursor, &report, export, bulk_base) {
                Ok(texture) => {
                    parsed += 1;
                    render(&mut lines, &texture, export);
                }
                Err(error) if matches!(error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::Unsupported) => {
                    refused += 1;
                    lines.push(format!("UNREAL_TEXTURE_REFUSED|{object}|{error}"));
                }
                Err(error) => return Err(error),
            }
        }
    }
    if total > MAX_TEXTURES {
        lines.push(format!("UNREAL_TEXTURE_TRUNCATED|{MAX_TEXTURES}|{total}"));
    }
    lines.push(format!("UNREAL_TEXTURE_SUMMARY|{total}|{processed}|{parsed}|{refused}"));
    source.unchanged()?;
    for line in &lines { println!("{line}"); }
    eprintln!("Read-only cooked UE4 Texture2D platform-data audit: verified window legacy -6/-7, UE4 459-484 only. Property tags, platform entries and mip bulk headers are parsed; mip payload bytes are range-checked and skipped, never decoded. Bulk offsets are contract-checked against the declared package summary fields; no dictionary, side-car file, writer or runtime compatibility is implied.");
    Ok(())
}

fn export_format(format: &str) -> Option<image_dds::ImageFormat> {
    use image_dds::ImageFormat::*;
    Some(match format {
        "PF_DXT1" => BC1RgbaUnorm,
        "PF_DXT3" => BC2RgbaUnorm,
        "PF_DXT5" => BC3RgbaUnorm,
        "PF_BC7" => BC7RgbaUnorm,
        "PF_BC4" => BC4RUnorm,
        "PF_BC5" => BC5RgUnorm,
        "PF_B8G8R8A8" => Bgra8Unorm,
        "PF_R8G8B8A8" => Rgba8Unorm,
        _ => return None,
    })
}

/// Development-only detached export of one inline cooked Texture2D into a
/// standalone DDS. Selection is explicit by export object name and must be
/// unique. Only an ordinary 2D platform entry (one format, `NumSlices` 1,
/// `FirstMipToSerialize` 0) whose whole mip chain is stored inline is accepted;
/// cubes, arrays, streamed/end-of-file bulk data, float/HDR/BC6H formats and
/// unknown shapes are refused rather than guessed. The source is never
/// modified. The inline payload is copied verbatim in mip order, and the written
/// DDS is re-parsed and its base mip decoded as a self-consistency check.
pub fn texture_export(name: &str, input: &Path, output: &Path) -> io::Result<()> {
    if name.is_empty() || name.len() > MAX_STRING || name.chars().any(|c| c.is_control() || c == '|') {
        return Err(invalid("invalid Unreal texture export name"));
    }
    match std::fs::symlink_metadata(output) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Unreal texture export destination exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut source = Source::open(input, MAX_FILE, EXPORT_LIMIT + 64 * 1024 * 1024)?;
    let report = unreal_package::inspect(&mut source)?;
    let bulk_base = unreal_package::bulk_data_start(&mut source, &report)?;
    let mut matches = Vec::new();
    for (index, export) in report.exports.iter().enumerate() {
        if report.class_name(export.class_index)? != "Texture2D" { continue; }
        if report.names[export.object_name] == name { matches.push(index); }
    }
    let export = match matches.as_slice() {
        [index] => &report.exports[*index],
        [] => return Err(invalid("no Texture2D export has this object name")),
        _ => return Err(invalid("more than one Texture2D export has this object name")),
    };
    let texture = {
        let mut cursor = Cursor::new(&mut source);
        parse_texture(&mut cursor, &report, export, bulk_base)?
    };
    let [entry] = texture.entries.as_slice() else {
        return Err(unsupported_text("Texture2D export has more than one platform entry"));
    };
    if entry.slices != 1 {
        return Err(unsupported_text("Texture2D platform entry is not a plain 2D image"));
    }
    if entry.first_mip != 0 {
        return Err(unsupported_text("Texture2D mip chain does not start at mip 0"));
    }
    if entry.width as u64 * entry.height as u64 > MAX_PIXELS {
        return Err(unsupported_text("Texture2D dimensions exceed the export limit"));
    }
    let format = export_format(&entry.format)
        .ok_or_else(|| unsupported_text(format!("pixel format {} is outside the export subset", entry.format)))?;
    if entry.mips.iter().any(|mip| mip.storage != Storage::Inline) {
        return Err(unsupported_text("Texture2D mip chain is not fully stored inline"));
    }
    let mut total = 0u64;
    for mip in &entry.mips {
        total = total.checked_add(mip.size_on_disk as u64)
            .ok_or_else(|| invalid("Unreal texture payload size overflow"))?;
        if mip.payload.is_none() { return Err(invalid("Unreal texture mip payload is unresolved")); }
    }
    if total == 0 || total > EXPORT_LIMIT {
        return Err(unsupported_text("Texture2D payload exceeds the export limit"));
    }
    let mut data = Vec::new();
    let mut cursor = Cursor::new(&mut source);
    for mip in &entry.mips {
        let (start, size) = mip.payload.unwrap();
        cursor.clear_limit();
        cursor.seek(start)?;
        cursor.set_limit(start + size)?;
        cursor.read_into(&mut data, size)?;
    }
    if data.len() as u64 != total { return Err(invalid("Unreal texture payload read a different length")); }
    let surface = image_dds::Surface {
        width: entry.width as u32,
        height: entry.height as u32,
        depth: 1,
        layers: 1,
        mipmaps: entry.mips.len() as u32,
        image_format: format,
        data: data.as_slice(),
    };
    let candidate = crate::texture::build_dds(&surface)?;
    // Re-parse the immutable candidate with the same DDS parser, then decode its
    // base mip. This checks self-consistency only, not independent agreement.
    let check = crate::texture::parse(&candidate)?;
    if check.width != entry.width as u32 || check.height != entry.height as u32
        || check.mipmaps != entry.mips.len() as u32 || check.format != format
        || &candidate[check.data_start..] != data.as_slice() {
        return Err(invalid("Unreal DDS export failed its self-check"));
    }
    {
        let decoded = image_dds::Surface {
            width: entry.width as u32,
            height: entry.height as u32,
            depth: 1,
            layers: 1,
            mipmaps: entry.mips.len() as u32,
            image_format: format,
            data: &candidate[check.data_start..],
        }
        .decode_rgba8()
        .map_err(|error| invalid(&format!("Unreal exported texture decode failed: {error}")))?;
        if decoded.width != entry.width as u32 || decoded.height != entry.height as u32
            || decoded.depth != 1 || decoded.layers != 1 {
            return Err(invalid("Unreal exported texture decoded to an unexpected shape"));
        }
    }
    super::detached_export::publish(output, &candidate, || source.unchanged())?;
    println!("UNREAL_TEXTURE_EXPORT|{name}|{}|{}|{}|{}|{}|{}|EXPORTED", entry.width, entry.height,
        entry.format, entry.mips.len(), total, candidate.len());
    eprintln!("Experimental detached cooked Texture2D export: source unchanged; inline payload copied verbatim in mip order. Only the verified UE4 459-484 window, plain 2D platform entries and fully inline mip chains are accepted; runtime compatibility, colour-space semantics and any package rebuild are unverified.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

    const MAGIC: u32 = 0x9E2A_83C1;

    fn names_for(format: &str) -> Vec<String> {
        ["/Game/Textures/example", "/Script/CoreUObject", "/Script/Engine", "example",
            "Class", "Texture2D", "Package", "ImportedSize", "IntPoint", "NeverStream",
            "StructProperty", "BoolProperty", "None", format]
            .iter().map(|name| (*name).to_string()).collect()
    }

    fn fstring(bytes: &mut Vec<u8>, text: &str) {
        bytes.extend(((text.len() + 1) as i32).to_le_bytes());
        bytes.extend(text.as_bytes());
        bytes.push(0);
    }

    fn fname(bytes: &mut Vec<u8>, index: i32) {
        bytes.extend(index.to_le_bytes());
        bytes.extend(0i32.to_le_bytes());
    }

    fn put_i32(bytes: &mut Vec<u8>, value: i32) {
        bytes.extend(value.to_le_bytes());
    }

    fn pattern(size: u64, seed: u8) -> Vec<u8> {
        (0..size).map(|index| (index as u8).wrapping_mul(7).wrapping_add(seed).wrapping_add(3)).collect()
    }

    struct FixtureMip {
        width: i32,
        height: i32,
        bulk: bool,
    }

    struct FixtureDesc<'a> {
        format: &'a str,
        width: i32,
        height: i32,
        slices: i32,
        first_mip: i32,
        cooked: u32,
        mips: Vec<FixtureMip>,
        duplicate_exports: bool,
        editor_package: bool,
    }

    impl<'a> FixtureDesc<'a> {
        fn new(format: &'a str, width: i32, height: i32, mips: Vec<FixtureMip>) -> Self {
            Self { format, width, height, slices: 1, first_mip: 0, cooked: 1, mips,
                duplicate_exports: false, editor_package: false }
        }
    }

    #[derive(Default)]
    struct MipFields {
        flags: usize,
        count: usize,
        size: usize,
        offset: usize,
        inline_data: Option<usize>,
    }

    struct Positions {
        prefix: usize,
        cooked: usize,
        skip_field: usize,
        never_stream_value: usize,
        mip_fields: Vec<MipFields>,
    }

    /// Build one LegacyFileVersion -6 / UE4 482 package with one Texture2D
    /// export whose serialized payload is a cooked platform entry. The summary
    /// tail mirrors the verified real-field chain so the declared bulk-data
    /// offset can be walked; table offsets are patched after layout.
    fn package(desc: &FixtureDesc) -> (Vec<u8>, Positions) {
        let names = names_for(desc.format);
        let index = |text: &str| names.iter().position(|name| name == text).unwrap() as i32;
        let mut bytes = Vec::new();
        bytes.extend(MAGIC.to_le_bytes());
        put_i32(&mut bytes, -6);
        bytes.extend(864u32.to_le_bytes());
        bytes.extend(482u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        let header_field = bytes.len();
        bytes.extend(0u32.to_le_bytes());
        fstring(&mut bytes, "None");
        bytes.extend(if desc.editor_package { 0u32 } else { 0x8000_0000u32 }.to_le_bytes());
        let name_fields = bytes.len();
        bytes.extend([0u8; 8]);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        let export_fields = bytes.len();
        bytes.extend([0u8; 8]);
        let import_fields = bytes.len();
        bytes.extend([0u8; 8]);
        let depends_field = bytes.len();
        bytes.extend(0u32.to_le_bytes());
        // Summary tail (after DependsOffset) up to BulkDataStartOffset.
        bytes.extend(0i32.to_le_bytes()); // StringAssetReferences count
        bytes.extend(0i32.to_le_bytes()); // StringAssetReferences offset
        bytes.extend(0i32.to_le_bytes()); // ThumbnailTableOffset
        bytes.extend([0u8; 16]); // package GUID
        bytes.extend(1i32.to_le_bytes()); // generation count
        bytes.extend(1i32.to_le_bytes()); // generation export count
        bytes.extend((names.len() as i32).to_le_bytes()); // generation name count
        for _ in 0..2 { // saved-by and compatible FEngineVersion, empty branches
            bytes.extend([0u8; 6]);
            put_i32(&mut bytes, 0);
            put_i32(&mut bytes, 0);
        }
        bytes.extend(0u32.to_le_bytes()); // CompressionFlags
        bytes.extend(0i32.to_le_bytes()); // CompressedChunks
        bytes.extend(0u32.to_le_bytes()); // PackageSource
        bytes.extend(0i32.to_le_bytes()); // AdditionalPackagesToCook
        bytes.extend(0i32.to_le_bytes()); // NumTextureAllocations
        bytes.extend(0i32.to_le_bytes()); // AssetRegistryDataOffset
        let bulk_field = bytes.len();
        bytes.extend([0u8; 8]); // BulkDataStartOffset (patched)
        bytes.extend(0i32.to_le_bytes()); // trailing summary words
        bytes.extend(0i32.to_le_bytes());
        let name_offset = bytes.len() as i32;
        for name in &names { fstring(&mut bytes, name); }
        let import_offset = bytes.len() as i32;
        for value in [index("/Script/CoreUObject"), 0, index("Class"), 0, -2, index("Texture2D"), 0] {
            put_i32(&mut bytes, value);
        }
        for value in [index("/Script/CoreUObject"), 0, index("Package"), 0, 0, index("/Script/Engine"), 0] {
            put_i32(&mut bytes, value);
        }
        let export_offset = bytes.len() as i32;
        let export_count = 1 + usize::from(desc.duplicate_exports);
        let mut serial_fields = Vec::new();
        for _ in 0..export_count {
            put_i32(&mut bytes, -1); // class: Texture2D import
            put_i32(&mut bytes, 0);
            put_i32(&mut bytes, 0);
            put_i32(&mut bytes, index("example"));
            put_i32(&mut bytes, 0);
            bytes.extend(0x0400_0003u32.to_le_bytes());
            let size = bytes.len();
            put_i32(&mut bytes, 0);
            let offset = bytes.len();
            put_i32(&mut bytes, 0);
            serial_fields.push((size, offset));
            bytes.extend([0u8; 12]);
            bytes.extend([0u8; 16]);
            bytes.extend(0u32.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
        }
        let depends_offset = bytes.len() as i32;
        bytes.extend([0u8; 8]); // depends table
        let header_size = bytes.len() as i32;
        let payload_offset = bytes.len() as u64;
        let (payload, positions, bulk) = class_data(desc, &names, payload_offset);
        let mut positions = positions;
        let payload_base = payload_offset as usize;
        positions.prefix += payload_base;
        positions.cooked += payload_base;
        positions.skip_field += payload_base;
        positions.never_stream_value += payload_base;
        for mip in &mut positions.mip_fields {
            mip.flags += payload_base;
            mip.count += payload_base;
            mip.size += payload_base;
            mip.offset += payload_base;
        }
        bytes[header_field..header_field + 4].copy_from_slice(&(header_size as u32).to_le_bytes());
        bytes[name_fields..name_fields + 4].copy_from_slice(&(names.len() as i32).to_le_bytes());
        bytes[name_fields + 4..name_fields + 8].copy_from_slice(&name_offset.to_le_bytes());
        bytes[export_fields..export_fields + 4].copy_from_slice(&(export_count as i32).to_le_bytes());
        bytes[export_fields + 4..export_fields + 8].copy_from_slice(&export_offset.to_le_bytes());
        bytes[import_fields..import_fields + 4].copy_from_slice(&2i32.to_le_bytes());
        bytes[import_fields + 4..import_fields + 8].copy_from_slice(&import_offset.to_le_bytes());
        bytes[depends_field..depends_field + 4].copy_from_slice(&depends_offset.to_le_bytes());
        bytes.extend_from_slice(&payload);
        let bulk_start = bytes.len() as u64;
        bytes.extend_from_slice(&bulk);
        bytes.extend(MAGIC.to_le_bytes());
        bytes[bulk_field..bulk_field + 8].copy_from_slice(&(bulk_start as i64).to_le_bytes());
        for (size, offset) in serial_fields {
            bytes[size..size + 4].copy_from_slice(&(payload.len() as i32).to_le_bytes());
            bytes[offset..offset + 4].copy_from_slice(&(payload_offset as i32).to_le_bytes());
        }
        (bytes, positions)
    }

    fn class_data(desc: &FixtureDesc, names: &[String], base: u64) -> (Vec<u8>, Positions, Vec<u8>) {
        let index = |text: &str| names.iter().position(|name| name == text).unwrap() as i32;
        let mut p = Vec::new();
        fname(&mut p, index("ImportedSize"));
        fname(&mut p, index("StructProperty"));
        put_i32(&mut p, 8);
        put_i32(&mut p, 0);
        fname(&mut p, index("IntPoint"));
        p.extend([0u8; 16]); // struct GUID
        put_i32(&mut p, desc.width);
        put_i32(&mut p, desc.height);
        fname(&mut p, index("NeverStream"));
        fname(&mut p, index("BoolProperty"));
        put_i32(&mut p, 0);
        put_i32(&mut p, 0);
        let never_stream_value = p.len();
        p.push(1);
        fname(&mut p, index("None"));
        let prefix = p.len();
        put_i32(&mut p, 0); // unexplained zero word, verified zero in all real samples
        p.extend([1u8, 0, 1, 0]); // two strip-flag pairs: editor data stripped
        let cooked = p.len();
        put_i32(&mut p, desc.cooked as i32);
        fname(&mut p, index(desc.format));
        let skip_field = p.len();
        put_i32(&mut p, 0);
        put_i32(&mut p, desc.width);
        put_i32(&mut p, desc.height);
        put_i32(&mut p, desc.slices);
        fstring(&mut p, desc.format);
        put_i32(&mut p, desc.first_mip);
        put_i32(&mut p, desc.mips.len() as i32);
        let mut mip_fields = Vec::new();
        let mut bulk = Vec::new();
        for mip in &desc.mips {
            let size = match layout_of(desc.format) {
                Some(layout) => mip_bytes(layout, mip.width, mip.height).unwrap(),
                None => 16,
            };
            put_i32(&mut p, 1); // per-mip cooked word
            let flags = p.len();
            put_i32(&mut p, if mip.bulk { BULK_PAYLOAD_AT_END as i32 } else { (BULK_FORCE_INLINE | BULK_SINGLE_USE) as i32 });
            let count = p.len();
            put_i32(&mut p, size as i32);
            let declared = p.len();
            put_i32(&mut p, size as i32);
            let offset = p.len();
            p.extend([0u8; 8]);
            let data_rel = p.len() as u64;
            let inline_data = if mip.bulk {
                p[offset..offset + 8].copy_from_slice(&(bulk.len() as i64).to_le_bytes());
                bulk.extend(pattern(size, bulk.len() as u8));
                None
            } else {
                p[offset..offset + 8].copy_from_slice(&((base + data_rel) as i64).to_le_bytes());
                p.extend(pattern(size, mip_fields.len() as u8));
                Some((base + data_rel) as usize)
            };
            put_i32(&mut p, mip.width);
            put_i32(&mut p, mip.height);
            mip_fields.push(MipFields { flags, count, size: declared, offset, inline_data });
        }
        let skip = base + p.len() as u64;
        p[skip_field..skip_field + 4].copy_from_slice(&(skip as i32).to_le_bytes());
        fname(&mut p, index("None"));
        (p, Positions { prefix, cooked, skip_field, never_stream_value, mip_fields }, bulk)
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let name = format!("bgc-unreal-texture-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
            let root = std::env::temp_dir().join(name);
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn write(&self, bytes: &[u8]) -> PathBuf {
            let path = self.0.join("texture.uasset");
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn parse_fixture(path: &Path) -> io::Result<Texture> {
        let mut source = Source::open(path, MAX_FILE, READ_BUDGET)?;
        let report = unreal_package::inspect(&mut source)?;
        let bulk_base = unreal_package::bulk_data_start(&mut source, &report)?;
        let mut cursor = Cursor::new(&mut source);
        for export in &report.exports {
            if report.class_name(export.class_index)? == "Texture2D" {
                return parse_texture(&mut cursor, &report, export, bulk_base);
            }
        }
        Err(invalid("fixture has no Texture2D export"))
    }

    fn one_mip(width: i32, height: i32) -> Vec<FixtureMip> {
        vec![FixtureMip { width, height, bulk: false }]
    }

    #[test]
    fn inline_texture_audits_and_exports_to_verified_dds() {
        let desc = FixtureDesc::new("PF_B8G8R8A8", 8, 8, one_mip(8, 8));
        let (bytes, positions) = package(&desc);
        let fixture = Fixture::new();
        let input = fixture.write(&bytes);
        let texture = parse_fixture(&input).unwrap();
        let entry = &texture.entries[0];
        assert_eq!((texture.object.as_str(), entry.format.as_str()), ("example", "PF_B8G8R8A8"));
        assert_eq!((entry.width, entry.height, entry.slices, entry.first_mip), (8, 8, 1, 0));
        assert_eq!((entry.mips.len(), entry.mips[0].size_on_disk), (1, 256));
        assert_eq!(entry.mips[0].storage, Storage::Inline);
        let start = positions.mip_fields[0].inline_data.unwrap() as u64;
        assert_eq!(entry.mips[0].payload, Some((start, 256)));
        // The detached export is byte-identical to the inline payload, never
        // rewrites the source and refuses an existing destination.
        let output = fixture.0.join("texture.dds");
        texture_export("example", &input, &output).unwrap();
        assert_eq!(fs::read(&input).unwrap(), bytes);
        let dds = fs::read(&output).unwrap();
        let parsed = crate::texture::parse(&dds).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.mipmaps), (8, 8, 1));
        assert_eq!(parsed.format, image_dds::ImageFormat::Bgra8Unorm);
        assert_eq!(&dds[parsed.data_start..], &bytes[start as usize..start as usize + 256]);
        assert_eq!(texture_export("example", &input, &output).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&input).unwrap(), bytes);
        // Unknown or unsafe export selectors are refused before publication.
        assert!(texture_export("missing", &input, &fixture.0.join("missing.dds")).is_err());
        assert!(texture_export("bad|name", &input, &fixture.0.join("bad.dds")).is_err());
    }

    #[test]
    fn multi_mip_chain_exports_in_declared_order() {
        let mips = vec![FixtureMip { width: 16, height: 16, bulk: false },
            FixtureMip { width: 8, height: 8, bulk: false }];
        let (bytes, positions) = package(&FixtureDesc::new("PF_DXT5", 16, 16, mips));
        let fixture = Fixture::new();
        let input = fixture.write(&bytes);
        let texture = parse_fixture(&input).unwrap();
        let entry = &texture.entries[0];
        assert_eq!((entry.mips.len(), entry.mips[0].size_on_disk, entry.mips[1].size_on_disk),
            (2, 256, 64));
        let output = fixture.0.join("chain.dds");
        texture_export("example", &input, &output).unwrap();
        let dds = fs::read(&output).unwrap();
        let parsed = crate::texture::parse(&dds).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.mipmaps), (16, 16, 2));
        let first = positions.mip_fields[0].inline_data.unwrap();
        let second = positions.mip_fields[1].inline_data.unwrap();
        let expected: Vec<u8> = bytes[first..first + 256].iter().chain(&bytes[second..second + 64])
            .copied().collect();
        assert_eq!(&dds[parsed.data_start..], expected.as_slice());
    }

    #[test]
    fn bulk_mips_resolve_against_the_summary_and_never_export() {
        let mips = vec![FixtureMip { width: 8, height: 8, bulk: true },
            FixtureMip { width: 4, height: 4, bulk: false }];
        let (bytes, positions) = package(&FixtureDesc::new("PF_BC5", 8, 8, mips));
        let fixture = Fixture::new();
        let input = fixture.write(&bytes);
        let texture = parse_fixture(&input).unwrap();
        let entry = &texture.entries[0];
        assert_eq!(entry.mips[0].storage, Storage::Bulk);
        assert_eq!(entry.mips[1].storage, Storage::Inline);
        let (start, size) = entry.mips[0].payload.unwrap();
        assert_eq!(size, 64);
        assert_eq!(&bytes[start as usize..start as usize + 64],
            &bytes[(bytes.len() - 4 - 64)..(bytes.len() - 4)]);
        assert_eq!(entry.mips[1].payload,
            Some((positions.mip_fields[1].inline_data.unwrap() as u64, 16)));
        // A mixed chain is audited but never exported.
        let error = texture_export("example", &input, &fixture.0.join("bulk.dds")).unwrap_err();
        assert!(error.to_string().contains("not fully stored inline"), "{error}");
        // Without a walkable summary the same texture still parses, but its bulk
        // payload stays explicitly unresolved instead of being guessed.
        let mut editor = FixtureDesc::new("PF_BC5", 8, 8,
            vec![FixtureMip { width: 8, height: 8, bulk: true },
                FixtureMip { width: 4, height: 4, bulk: false }]);
        editor.editor_package = true;
        let (editor_bytes, _) = package(&editor);
        let editor_input = fixture.write(&editor_bytes);
        let editor_texture = parse_fixture(&editor_input).unwrap();
        assert_eq!(editor_texture.entries[0].mips[0].payload, None);
        assert_eq!(editor_texture.entries[0].mips[0].storage, Storage::Bulk);
    }

    #[test]
    fn cube_float_and_unknown_formats_are_refused_for_export() {
        let fixture = Fixture::new();
        // NumSlices 6 marks a cubemap; only a plain 2D image may be exported.
        let mut cube = FixtureDesc::new("PF_B8G8R8A8", 8, 8, one_mip(8, 8));
        cube.slices = 6;
        let (cube_bytes, _) = package(&cube);
        let cube_input = fixture.write(&cube_bytes);
        let error = texture_export("example", &cube_input, &fixture.0.join("cube.dds")).unwrap_err();
        assert!(error.to_string().contains("plain 2D image"), "{error}");
        // Float formats are audited (their layout is known) but never exported.
        let (float_bytes, _) = package(&FixtureDesc::new("PF_A32B32G32R32F", 2, 2, one_mip(2, 2)));
        let float_input = fixture.write(&float_bytes);
        let float_texture = parse_fixture(&float_input).unwrap();
        assert_eq!(float_texture.entries[0].mips[0].size_on_disk, 64);
        let error = texture_export("example", &float_input, &fixture.0.join("float.dds")).unwrap_err();
        assert!(error.to_string().contains("outside the export subset"), "{error}");
        // Formats without a verified byte layout are reported without a size
        // claim and are never exported.
        let (other_bytes, _) = package(&FixtureDesc::new("PF_Other", 4, 4, one_mip(4, 4)));
        let other_input = fixture.write(&other_bytes);
        let other_texture = parse_fixture(&other_input).unwrap();
        assert_eq!(layout_of(&other_texture.entries[0].format), None);
        assert!(texture_export("example", &other_input, &fixture.0.join("other.dds")).is_err());
        // A non-zero FirstMipToSerialize is outside the export subset.
        let mut shifted = FixtureDesc::new("PF_B8G8R8A8", 8, 8, one_mip(8, 8));
        shifted.first_mip = 2;
        let (shifted_bytes, _) = package(&shifted);
        let shifted_input = fixture.write(&shifted_bytes);
        let error = texture_export("example", &shifted_input, &fixture.0.join("shifted.dds")).unwrap_err();
        assert!(error.to_string().contains("does not start at mip 0"), "{error}");
    }

    #[test]
    fn malformed_payloads_are_refused_without_guessing() {
        let fixture = Fixture::new();
        let (bytes, positions) = package(&FixtureDesc::new("PF_B8G8R8A8", 8, 8, one_mip(8, 8)));
        let mutations: [(&str, fn(&mut Vec<u8>, &Positions)); 7] = [
            ("not cooked", |bytes, positions| put_i32_at(bytes, positions.cooked, 0)),
            ("prefix", |bytes, positions| put_i32_at(bytes, positions.prefix, 1)),
            ("skip offset", |bytes, positions| put_i32_at(bytes, positions.skip_field, 0)),
            ("inline offset", |bytes, positions| {
                let offset = positions.mip_fields[0].offset;
                put_i32_at(bytes, offset, 0);
                bytes[offset + 4..offset + 8].copy_from_slice(&0u32.to_le_bytes());
            }),
            ("declared size", |bytes, positions| {
                put_i32_at(bytes, positions.mip_fields[0].size, 257)
            }),
            ("element count", |bytes, positions| {
                put_i32_at(bytes, positions.mip_fields[0].count, 255)
            }),
            ("unknown bulk flags", |bytes, positions| {
                put_i32_at(bytes, positions.mip_fields[0].flags, 0x4000)
            }),
        ];
        for (label, mutate) in mutations {
            let mut mutated = bytes.clone();
            mutate(&mut mutated, &positions);
            let path = fixture.write(&mutated);
            assert!(parse_fixture(&path).is_err(), "{label} must be refused");
        }
        // A bool property value must stay 0/1.
        let mut mutated = bytes.clone();
        mutated[positions.never_stream_value] = 2;
        let path = fixture.write(&mutated);
        assert!(parse_fixture(&path).is_err());
    }

    fn put_i32_at(bytes: &mut [u8], offset: usize, value: i32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn codec_subset_exports_to_matching_dds_formats() {
        let fixture = Fixture::new();
        for format in ["PF_DXT1", "PF_DXT3", "PF_DXT5", "PF_BC7", "PF_BC4", "PF_BC5",
            "PF_B8G8R8A8", "PF_R8G8B8A8"] {
            let (bytes, _) = package(&FixtureDesc::new(format, 8, 8, one_mip(8, 8)));
            let input = fixture.write(&bytes);
            let output = fixture.0.join("codec.dds");
            let _ = fs::remove_file(&output);
            texture_export("example", &input, &output).unwrap_or_else(|error| {
                panic!("{format} export failed: {error}")
            });
            let dds = fs::read(&output).unwrap();
            let parsed = crate::texture::parse(&dds).unwrap();
            assert_eq!(parsed.format, export_format(format).unwrap(), "{format}");
            assert_eq!((parsed.width, parsed.height, parsed.mipmaps), (8, 8, 1), "{format}");
            let legacy = matches!(parsed.format,
                image_dds::ImageFormat::BC1RgbaUnorm | image_dds::ImageFormat::BC2RgbaUnorm
                | image_dds::ImageFormat::BC3RgbaUnorm);
            assert_eq!(parsed.data_start, if legacy { 128 } else { 148 }, "{format}");
        }
    }

    #[test]
    fn duplicated_export_names_are_refused() {
        let mut desc = FixtureDesc::new("PF_B8G8R8A8", 8, 8, one_mip(8, 8));
        desc.duplicate_exports = true;
        let (bytes, _) = package(&desc);
        let fixture = Fixture::new();
        let input = fixture.write(&bytes);
        let error = texture_export("example", &input, &fixture.0.join("ambiguous.dds")).unwrap_err();
        assert!(error.to_string().contains("more than one"), "{error}");
    }

    #[test]
    fn audit_reads_a_regular_fixture_and_refuses_symlinks() {
        use std::os::unix::fs::symlink;
        let (bytes, _) = package(&FixtureDesc::new("PF_B8G8R8A8", 4, 4, one_mip(4, 4)));
        let fixture = Fixture::new();
        let path = fixture.write(&bytes);
        audit(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let linked = fixture.0.join("linked.uasset");
        symlink("texture.uasset", &linked).unwrap();
        assert!(audit(&linked).is_err());
        assert!(audit(&fixture.0.join("missing.uasset")).is_err());
    }
}
