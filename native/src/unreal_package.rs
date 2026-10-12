//! Development-only, read-only Unreal Engine 4 cooked `.uasset` header audit.
//! Parses the package summary prefix, name table and export table for a narrow,
//! empirically verified LegacyFileVersion -6/-7 layout window (UE4 file version
//! 459-484). Cooked property tags, TexturePlatformData, mip/pixel payloads and
//! bulk data are never parsed; export serial sizes/offsets are declared fields,
//! not verified extents. Version gates were cross-checked against the public
//! UE4 `FPackageFileSummary`/`FObjectExport` serializers and UEViewer's
//! UE4Version.h constant list (documentation only, no runtime dependency).
use std::{io, path::Path};

use super::{audit_io::Source, invalid};

const MAGIC: u32 = 0x9E2A_83C1;
const LEGACY_UE3_VERSION: u32 = 864;
/// `VER_UE4_SERIALIZE_TEXT_IN_PACKAGES`: GatherableTextDataCount/Offset exist.
const MIN_UE4: u32 = 459;
/// Last version before `VER_UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT` (485) adds
/// `bIsAsset`, before name hashes (504), cooked-export template/preload fields
/// (507/508) and 64-bit export map sizes (511).
const MAX_UE4: u32 = 484;
const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;
const READ_BUDGET: u64 = 256 * 1024 * 1024;
const MAX_RECORDS: i32 = 1_000_000;
const MAX_TABLE: u64 = 64 * 1024 * 1024;
const MAX_STRING: usize = 4096;
const MAX_REPORTED_TEXTURES: usize = 200;
/// FObjectExport for the supported window: seven 32-bit index/size fields, the
/// FName, ObjectFlags, two 32-bit size fields, three 4-byte bools, the package
/// GUID, PackageFlags and bNotAlwaysLoadedForEditorGame.
const EXPORT_RECORD: u64 = 68;
const IMPORT_RECORD: u64 = 28;
const WINDOW: usize = 64 * 1024;

fn unsupported(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}

/// Sequential little-endian reader with a bounded refill window. Every offset is
/// checked against the opened source length; actual reads stay inside the shared
/// `Source` read budget.
struct Reader<'a> {
    source: &'a mut Source,
    length: u64,
    position: u64,
    start: u64,
    window: Vec<u8>,
}

impl<'a> Reader<'a> {
    fn new(source: &'a mut Source) -> Self {
        let length = source.len();
        Self { source, length, position: 0, start: 0, window: Vec::new() }
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, position: u64) -> io::Result<()> {
        if position > self.length { return Err(invalid("Unreal package offset beyond source")); }
        self.position = position;
        Ok(())
    }

    fn ensure(&mut self, count: usize) -> io::Result<()> {
        let end = self.position.checked_add(count as u64)
            .ok_or_else(|| invalid("Unreal package offset overflow"))?;
        if self.position >= self.start
            && self.start.checked_add(self.window.len() as u64).is_some_and(|window| end <= window) {
            return Ok(());
        }
        let available = self.length.checked_sub(self.position)
            .ok_or_else(|| invalid("Unreal package offset beyond source"))?;
        let want = (count.max(WINDOW) as u64).min(available) as usize;
        if want < count { return Err(invalid("truncated Unreal package table")); }
        self.window = self.source.read_at(self.position, want)?;
        self.start = self.position;
        Ok(())
    }

    fn take(&mut self, count: usize) -> io::Result<&[u8]> {
        self.ensure(count)?;
        let begin = usize::try_from(self.position - self.start)
            .map_err(|_| invalid("Unreal package offset overflow"))?;
        let end = begin.checked_add(count).ok_or_else(|| invalid("Unreal package offset overflow"))?;
        self.position += count as u64;
        self.window.get(begin..end).ok_or_else(|| invalid("truncated Unreal package table"))
    }

    fn skip(&mut self, count: usize) -> io::Result<()> {
        self.ensure(count)?;
        self.position += count as u64;
        Ok(())
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Boolean fields in this window are written as 32-bit 0/1 words. Any other
    /// encoding means the record layout is not the verified one.
    fn flag(&mut self) -> io::Result<()> {
        if self.u32()? > 1 { return Err(invalid("Unsupported Unreal export flag encoding")); }
        Ok(())
    }

    fn string(&mut self) -> io::Result<String> {
        let size = self.i32()?;
        let count = size.checked_abs().ok_or_else(|| invalid("invalid Unreal string size"))? as usize;
        if count == 0 || count > MAX_STRING { return Err(invalid("Unreal string exceeds bounds")); }
        let text = if size > 0 {
            let bytes = self.take(count)?;
            if bytes[count - 1] != 0 { return Err(invalid("Unreal string is not terminated")); }
            std::str::from_utf8(&bytes[..count - 1])
                .map_err(|_| invalid("invalid Unreal UTF-8"))?.to_owned()
        } else {
            let bytes = self.take(count * 2)?;
            let values: Vec<u16> = bytes.chunks_exact(2)
                .map(|pair| u16::from_le_bytes(pair.try_into().unwrap())).collect();
            if values[count - 1] != 0 { return Err(invalid("Unreal UTF-16 string is not terminated")); }
            String::from_utf16(&values[..count - 1]).map_err(|_| invalid("invalid Unreal UTF-16"))?
        };
        if text.chars().any(|c| c.is_control() || c == '|') { return Err(invalid("unsafe Unreal package text")); }
        Ok(text)
    }
}

#[derive(Debug)]
pub(crate) struct Export {
    pub class_index: i32,
    pub outer_index: i32,
    pub object_name: usize,
    pub serial_size: i64,
    pub serial_offset: i64,
}

#[derive(Debug)]
pub(crate) struct Report {
    pub legacy: i32,
    pub ue4: u32,
    pub package_name: String,
    pub package_flags: u32,
    pub total_header_size: i32,
    pub name_offset: i32,
    pub import_offset: i32,
    pub export_offset: i32,
    pub depends_offset: i32,
    pub names: Vec<String>,
    /// Import object-name indexes, used to resolve negative export class indexes.
    pub import_classes: Vec<usize>,
    pub exports: Vec<Export>,
}

impl Report {
    /// Resolve an export class index to a name table entry. Negative indexes name
    /// an imported object (`-index - 1` into the import map); positive indexes
    /// name an export in the same package (`index - 1`). Zero is `None`.
    fn class_name(&self, index: i32) -> io::Result<&str> {
        if index == 0 { return Ok("None"); }
        let name_index = if index < 0 {
            let import = index.checked_neg().and_then(|value| value.checked_sub(1))
                .and_then(|value| usize::try_from(value).ok())
                .and_then(|value| self.import_classes.get(value));
            *import.ok_or_else(|| invalid("Unreal export class import index is out of range"))?
        } else {
            usize::try_from(index - 1).ok().and_then(|value| self.exports.get(value))
                .map(|export| export.object_name)
                .ok_or_else(|| invalid("Unreal export class export index is out of range"))?
        };
        self.names.get(name_index).map(String::as_str)
            .ok_or_else(|| invalid("Unreal export class name index is out of range"))
    }
}

fn table_count(value: i32, message: &'static str) -> io::Result<u32> {
    if !(0..=MAX_RECORDS).contains(&value) { return Err(invalid(message)); }
    Ok(value as u32)
}

fn table_offset(value: i32, header_size: u64) -> io::Result<u64> {
    if value < 0 || value as u64 > header_size { return Err(invalid("Unreal package table offset outside header")); }
    Ok(value as u64)
}

fn inspect(source: &mut Source) -> io::Result<Report> {
    let mut reader = Reader::new(source);
    if reader.u32()? != MAGIC { return Err(invalid("not an Unreal package (bad magic)")); }
    let legacy = reader.i32()?;
    if legacy != -6 && legacy != -7 {
        return Err(unsupported("Unreal package legacy file version is unsupported; this audit knows -6/-7 only"));
    }
    if reader.u32()? != LEGACY_UE3_VERSION {
        return Err(unsupported("Unreal package UE3 version is unsupported; this audit knows the UE4 constant 864 only"));
    }
    let ue4 = reader.u32()?;
    if !(MIN_UE4..=MAX_UE4).contains(&ue4) {
        return Err(unsupported("Unreal package UE4 file version is unsupported; this audit knows 459-484 only"));
    }
    if reader.u32()? != 0 {
        return Err(unsupported("Unreal package licensee version is unsupported; this audit knows 0 only"));
    }
    if reader.u32()? != 0 {
        return Err(unsupported("Unreal package custom version containers are unsupported"));
    }
    let total_header_size = reader.i32()?;
    if total_header_size < 0 || total_header_size as u64 > reader.length {
        return Err(invalid("Unreal package header size outside source"));
    }
    let header_size = total_header_size as u64;
    let package_name = reader.string()?;
    let package_flags = reader.u32()?;
    let name_count = table_count(reader.i32()?, "Unreal package name count exceeds limit")?;
    let name_offset = table_offset(reader.i32()?, header_size)?;
    let gatherable_count = reader.i32()?;
    let gatherable_offset = reader.i32()?;
    if !(0..=MAX_RECORDS).contains(&gatherable_count) {
        return Err(invalid("Unreal package gatherable-text count exceeds limit"));
    }
    if gatherable_offset < 0 || gatherable_offset as u64 > reader.length {
        return Err(invalid("Unreal package gatherable-text offset outside source"));
    }
    let export_count = table_count(reader.i32()?, "Unreal package export count exceeds limit")?;
    let export_offset = table_offset(reader.i32()?, header_size)?;
    let import_count = table_count(reader.i32()?, "Unreal package import count exceeds limit")?;
    let import_offset = table_offset(reader.i32()?, header_size)?;
    let depends_offset = table_offset(reader.i32()?, header_size)?;

    let export_bytes = u64::from(export_count) * EXPORT_RECORD;
    if export_bytes > MAX_TABLE
        || export_offset.checked_add(export_bytes).is_none_or(|end| end > header_size) {
        return Err(invalid("Unreal package export table exceeds its bounds"));
    }
    let import_bytes = u64::from(import_count) * IMPORT_RECORD;
    if import_bytes > MAX_TABLE
        || import_offset.checked_add(import_bytes).is_none_or(|end| end > header_size) {
        return Err(invalid("Unreal package import table exceeds its bounds"));
    }

    let mut names = Vec::new();
    let mut name_bytes = 0u64;
    if name_count > 0 {
        reader.seek(name_offset)?;
        for _ in 0..name_count {
            let begin = reader.position();
            names.push(reader.string()?);
            name_bytes = name_bytes.checked_add(reader.position() - begin)
                .ok_or_else(|| invalid("Unreal package name table size overflow"))?;
            if name_bytes > MAX_TABLE { return Err(invalid("Unreal package name table exceeds limit")); }
        }
        if reader.position() > header_size { return Err(invalid("Unreal package name table exceeds header")); }
        let mut sorted: Vec<&str> = names.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("duplicate Unreal package name"));
        }
    }

    let mut import_classes = Vec::new();
    if import_count > 0 {
        reader.seek(import_offset)?;
        for _ in 0..import_count {
            // FObjectImport: two FNames (index + number), an outer package index
            // and the object FName (index + number).
            let class_package = reader.i32()?;
            reader.i32()?; // ClassPackage name number
            let class_name = reader.i32()?;
            reader.i32()?; // ClassName name number
            reader.i32()?; // OuterIndex
            let object_name = reader.i32()?;
            reader.i32()?; // ObjectName name number
            for index in [class_package, class_name, object_name] {
                if index < 0 || index as u32 >= name_count {
                    return Err(invalid("Unreal import name index is out of range"));
                }
            }
            import_classes.push(object_name as usize);
        }
    }

    let mut exports = Vec::new();
    if export_count > 0 {
        reader.seek(export_offset)?;
        for _ in 0..export_count {
            let class_index = reader.i32()?;
            let _super_index = reader.i32()?;
            let outer_index = reader.i32()?;
            let object_name = reader.i32()?;
            let _object_number = reader.i32()?;
            let _object_flags = reader.u32()?;
            let serial_size = i64::from(reader.i32()?);
            let serial_offset = i64::from(reader.i32()?);
            reader.flag()?; // bForcedExport
            reader.flag()?; // bNotForClient
            reader.flag()?; // bNotForServer
            reader.skip(16)?; // PackageGuid
            let _package_flags = reader.u32()?;
            reader.flag()?; // bNotAlwaysLoadedForEditorGame
            if object_name < 0 || object_name as u32 >= name_count {
                return Err(invalid("Unreal export name index is out of range"));
            }
            if serial_size < 0 || serial_offset < 0
                || (serial_offset as u64).checked_add(serial_size as u64).is_none_or(|end| end > reader.length) {
                return Err(invalid("Unreal export extent exceeds source bounds"));
            }
            exports.push(Export { class_index, outer_index, object_name: object_name as usize,
                serial_size, serial_offset });
        }
    }

    let report = Report { legacy, ue4, package_name, package_flags, total_header_size,
        name_offset: name_offset as i32, import_offset: import_offset as i32,
        export_offset: export_offset as i32, depends_offset: depends_offset as i32,
        names, import_classes, exports };
    for export in &report.exports { report.class_name(export.class_index)?; }
    Ok(report)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, MAX_FILE, READ_BUDGET)?;
    let report = inspect(&mut source)?;
    source.unchanged()?;
    println!("UNREAL_PACKAGE|{}|{}|{}|{}|{}", report.legacy, report.ue4, report.names.len(),
        report.exports.len(), report.import_classes.len());
    println!("UNREAL_PACKAGE_NAME|{}", report.package_name);
    println!("UNREAL_PACKAGE_FLAGS|{:#010X}", report.package_flags);
    println!("UNREAL_PACKAGE_TABLES|{}|{}|{}|{}|{}", report.total_header_size,
        report.name_offset, report.import_offset, report.export_offset, report.depends_offset);
    let mut textures = 0usize;
    for export in &report.exports {
        if report.class_name(export.class_index)? != "Texture2D" { continue; }
        textures += 1;
        if textures <= MAX_REPORTED_TEXTURES {
            println!("UNREAL_PACKAGE_EXPORT|{}|{}|{}|{}|{}", report.names[export.object_name],
                "Texture2D", export.serial_size, export.serial_offset, export.outer_index);
        }
    }
    if textures > MAX_REPORTED_TEXTURES {
        println!("UNREAL_PACKAGE_EXPORT_TRUNCATED|{}|{}", MAX_REPORTED_TEXTURES, textures);
    }
    println!("UNREAL_PACKAGE_TEXTURES|{textures}");
    println!("UNREAL_PACKAGE_EXPORTS|{}", report.exports.len());
    eprintln!("Read-only Unreal package header audit: summary, name and export tables only. Cooked property tags, TexturePlatformData, mip/pixel payloads, bulk data and the depends table are never parsed; declared serial sizes/offsets are not verified extents. No Unreal package writer exists, and this does not prove integrity or runtime compatibility.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

    const DEFAULT_NAMES: [&str; 7] = [
        "/Game/Textures/example", "/Script/CoreUObject", "/Script/Engine", "example",
        "Class", "Texture2D", "Package",
    ];

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let name = format!("bgc-unreal-package-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
            let root = std::env::temp_dir().join(name);
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn write(&self, bytes: &[u8]) -> PathBuf {
            let path = self.0.join("package.uasset");
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn inspect_file(fixture: &Fixture, bytes: &[u8]) -> io::Result<Report> {
        let path = fixture.write(bytes);
        let mut source = Source::open(&path, MAX_FILE, READ_BUDGET)?;
        inspect(&mut source)
    }

    fn inspect_bytes(bytes: &[u8]) -> io::Result<Report> {
        inspect_file(&Fixture::new(), bytes)
    }

    fn fstring(bytes: &mut Vec<u8>, text: &str) {
        bytes.extend(((text.len() + 1) as i32).to_le_bytes());
        bytes.extend(text.as_bytes());
        bytes.push(0);
    }

    /// Build one LegacyFileVersion -6 / UE4 482 package with two imports (a
    /// `Class`/`Texture2D` class import and an engine package import) and one
    /// export. Table offsets are patched after the body is laid out so the
    /// fixture mirrors the verified on-disk order (summary, names, imports,
    /// exports, depends table, payload).
    fn fixture(names: &[&str]) -> Vec<u8> {
        let index = |text: &str| names.iter().position(|name| *name == text).map_or(0, |i| i as i32);
        let mut bytes = Vec::new();
        bytes.extend(MAGIC.to_le_bytes());
        bytes.extend((-6i32).to_le_bytes());
        bytes.extend(LEGACY_UE3_VERSION.to_le_bytes());
        bytes.extend(482u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes()); // licensee version
        bytes.extend(0u32.to_le_bytes()); // empty custom version container
        bytes.extend(0u32.to_le_bytes()); // total header size (patched)
        fstring(&mut bytes, "None");
        bytes.extend(0x8000_0000u32.to_le_bytes()); // PKG_FilterEditorOnly
        let name_fields = bytes.len();
        bytes.extend([0u8; 8]);
        bytes.extend(0u32.to_le_bytes()); // gatherable text count
        bytes.extend(0u32.to_le_bytes()); // gatherable text offset
        let export_fields = bytes.len();
        bytes.extend([0u8; 8]);
        let import_fields = bytes.len();
        bytes.extend([0u8; 8]);
        let depends_field = bytes.len();
        bytes.extend(0u32.to_le_bytes());
        let name_offset = bytes.len() as i32;
        for name in names { fstring(&mut bytes, name); }
        let import_offset = bytes.len() as i32;
        bytes.extend(index("/Script/CoreUObject").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ClassPackage name number
        bytes.extend(index("Class").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ClassName name number
        bytes.extend((-2i32).to_le_bytes()); // outer package index
        bytes.extend(index("Texture2D").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ObjectName name number
        bytes.extend(index("/Script/CoreUObject").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ClassPackage name number
        bytes.extend(index("Package").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ClassName name number
        bytes.extend(0i32.to_le_bytes()); // outer root
        bytes.extend(index("/Script/Engine").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // ObjectName name number
        let export_offset = bytes.len() as i32;
        bytes.extend((-1i32).to_le_bytes()); // class index: import 0
        bytes.extend(0i32.to_le_bytes()); // super index
        bytes.extend(0i32.to_le_bytes()); // outer index
        bytes.extend(index("example").to_le_bytes());
        bytes.extend(0i32.to_le_bytes()); // object number
        bytes.extend(0x0400_0003u32.to_le_bytes());
        bytes.extend(16i32.to_le_bytes()); // serial size
        let serial_offset = bytes.len();
        bytes.extend(0i32.to_le_bytes()); // serial offset (patched)
        bytes.extend([0u8; 12]); // three 4-byte bools
        bytes.extend([0u8; 16]); // package guid
        bytes.extend(0u32.to_le_bytes()); // export package flags
        bytes.extend(1u32.to_le_bytes()); // bNotAlwaysLoadedForEditorGame
        let depends_offset = bytes.len() as i32;
        bytes.extend([0u8; 8]); // depends table
        let total_header_size = bytes.len() as i32;
        let payload_offset = bytes.len() as i32;
        bytes.extend([0u8; 16]); // declared export payload
        bytes[24..28].copy_from_slice(&(total_header_size as u32).to_le_bytes());
        bytes[name_fields..name_fields + 4].copy_from_slice(&(names.len() as i32).to_le_bytes());
        bytes[name_fields + 4..name_fields + 8].copy_from_slice(&name_offset.to_le_bytes());
        bytes[export_fields..export_fields + 4].copy_from_slice(&1i32.to_le_bytes());
        bytes[export_fields + 4..export_fields + 8].copy_from_slice(&export_offset.to_le_bytes());
        bytes[import_fields..import_fields + 4].copy_from_slice(&2i32.to_le_bytes());
        bytes[import_fields + 4..import_fields + 8].copy_from_slice(&import_offset.to_le_bytes());
        bytes[depends_field..depends_field + 4].copy_from_slice(&depends_offset.to_le_bytes());
        bytes[serial_offset..serial_offset + 4].copy_from_slice(&payload_offset.to_le_bytes());
        bytes
    }

    fn patch(bytes: &mut [u8], offset: usize, value: i32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn minimal_package_reports_the_name_and_export_tables() {
        let report = inspect_bytes(&fixture(&DEFAULT_NAMES)).unwrap();
        assert_eq!((report.legacy, report.ue4), (-6, 482));
        assert!(report.total_header_size > 0);
        assert_eq!(report.package_name, "None");
        assert_eq!(report.package_flags, 0x8000_0000);
        assert_eq!(report.names, DEFAULT_NAMES);
        assert_eq!(report.import_classes, [5, 2]);
        assert_eq!(report.export_offset - report.import_offset, 2 * IMPORT_RECORD as i32);
        assert_eq!(report.exports.len(), 1);
        let export = &report.exports[0];
        assert_eq!((export.class_index, export.outer_index, export.object_name), (-1, 0, 3));
        assert_eq!((export.serial_size, export.serial_offset),
            (16, i64::from(report.total_header_size)));
        assert_eq!(report.class_name(-1).unwrap(), "Texture2D");
        assert_eq!(report.class_name(0).unwrap(), "None");
        assert_eq!(report.depends_offset + 8, report.total_header_size);
    }

    #[test]
    fn truncated_packages_fail_without_panicking() {
        let full = fixture(&DEFAULT_NAMES);
        let fixture = Fixture::new();
        for length in 0..full.len() {
            assert!(inspect_file(&fixture, &full[..length]).is_err(), "truncated at {length}");
        }
    }

    #[test]
    fn unsupported_versions_and_custom_containers_are_refused() {
        let full = fixture(&DEFAULT_NAMES);
        for (offset, value) in [(4usize, -5i32), (4, -8), (4, -1)] {
            let mut bytes = full.clone();
            patch(&mut bytes, offset, value);
            assert_eq!(inspect_bytes(&bytes).unwrap_err().kind(), io::ErrorKind::Unsupported, "legacy {value}");
        }
        for (offset, value) in [(8usize, 863u32), (12, 458), (12, 485), (16, 1), (20, 1)] {
            let mut bytes = full.clone();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(inspect_bytes(&bytes).unwrap_err().kind(), io::ErrorKind::Unsupported, "field at {offset}");
        }
    }

    #[test]
    fn oversized_duplicate_and_unsafe_strings_are_refused() {
        let full = fixture(&DEFAULT_NAMES);
        let name_offset = i32::from_le_bytes(full[45..49].try_into().unwrap()) as usize;
        let mut bytes = full.clone();
        patch(&mut bytes, name_offset, MAX_STRING as i32 + 1); // first name length
        assert!(inspect_bytes(&bytes).is_err());
        assert!(inspect_bytes(&fixture(&["duplicate", "duplicate", "Texture2D", "example"])).is_err());
        assert!(inspect_bytes(&fixture(&["unsafe|name"])).is_err());
        assert!(inspect_bytes(&fixture(&["unsafe\nname"])).is_err());
        let mut bytes = full.clone();
        patch(&mut bytes, 41, MAX_RECORDS + 1); // name count
        assert!(inspect_bytes(&bytes).is_err());
    }

    #[test]
    fn out_of_bounds_offsets_counts_and_indexes_are_refused() {
        let full = fixture(&DEFAULT_NAMES);
        let header = i32::from_le_bytes(full[24..28].try_into().unwrap());
        let export_offset = i32::from_le_bytes(full[61..65].try_into().unwrap()) as usize;
        // Name table offset outside the declared header.
        let mut bytes = full.clone();
        patch(&mut bytes, 45, header + 4);
        assert!(inspect_bytes(&bytes).is_err());
        // Name table offset beyond the source.
        let mut bytes = full.clone();
        patch(&mut bytes, 45, full.len() as i32 + 4);
        assert!(inspect_bytes(&bytes).is_err());
        // Export table larger than the declared header.
        let mut bytes = full.clone();
        patch(&mut bytes, 61, header);
        assert!(inspect_bytes(&bytes).is_err());
        // Export extent beyond the source.
        let mut bytes = full.clone();
        patch(&mut bytes, export_offset + 28, full.len() as i32);
        assert!(inspect_bytes(&bytes).is_err());
        // Export name index outside the name table.
        let mut bytes = full.clone();
        patch(&mut bytes, export_offset + 12, DEFAULT_NAMES.len() as i32);
        assert!(inspect_bytes(&bytes).is_err());
        // Export class index outside the import map.
        let mut bytes = full.clone();
        patch(&mut bytes, export_offset, -3);
        assert!(inspect_bytes(&bytes).is_err());
        // Export boolean fields must be 0/1 words.
        let mut bytes = full.clone();
        patch(&mut bytes, export_offset + 32, 2);
        assert!(inspect_bytes(&bytes).is_err());
    }

    #[test]
    fn audit_reads_a_regular_fixture_and_refuses_symlinks() {
        let fixture = Fixture::new();
        let path = fixture.write(&fixture_bytes());
        audit(&path).unwrap();
        let linked = fixture.0.join("linked.uasset");
        symlink("package.uasset", &linked).unwrap();
        assert!(audit(&linked).is_err());
        assert!(audit(&fixture.0.join("missing.uasset")).is_err());
    }

    fn fixture_bytes() -> Vec<u8> { fixture(&DEFAULT_NAMES) }
}
