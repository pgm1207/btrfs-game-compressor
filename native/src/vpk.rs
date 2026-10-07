//! Development-only Valve VPK v1/v2 directory and declared-extent inventory.
//! No extraction, numbered companion opening, CRC/MD5/signature verification,
//! VTF/VTEX payload parsing or writer. Respawn's custom VPK stays unsupported.
use std::{collections::{BTreeMap, BTreeSet}, io, path::Path};
use super::{audit_io::{field, le32, Source}, invalid};

const MAGIC: u32 = 0x55aa1234;
const MAX_TREE: u32 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 65536;
const MAX_REPORTS: usize = 4096;
const MAX_RETAINED_NAMES: usize = 8 * 1024 * 1024;

struct Cursor<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or_else(|| invalid("VPK directory offset overflow"))?;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| invalid("truncated VPK directory"))?;
        self.pos = end;
        Ok(bytes)
    }
    fn text(&mut self) -> io::Result<Vec<u8>> {
        let tail = self.bytes.get(self.pos..).ok_or_else(|| invalid("invalid VPK string offset"))?;
        let len = tail.iter().take(4097).position(|b| *b == 0)
            .ok_or_else(|| invalid("VPK string is unterminated or exceeds audit limit"))?;
        Ok(self.take(len + 1)?[..len].to_vec())
    }
    fn u16(&mut self) -> io::Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap())) }
    fn u32(&mut self) -> io::Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
}

struct Header {
    version: u32, size: u64, tree_size: u32, data_start: u64, data_size: u64,
    archive_md5: u32, other_md5: u32, signature: u32, trailing: u64,
}
struct Entry { path: Vec<u8>, crc: u32, preload_offset: u64, preload_size: u16,
    archive: u16, offset: u32, length: u32, path_status: &'static str }
struct Inventory {
    header: Header, entries: Vec<Entry>, extensions: BTreeMap<Vec<u8>, (u64, u64)>,
    archives: BTreeMap<u16, u64>, logical_bytes: u64,
}

fn header(source: &mut Source) -> io::Result<Header> {
    let bytes = source.read_at(0, 12)?;
    if le32(&bytes, 0)? != MAGIC { return Err(invalid("not a Valve VPK directory header")); }
    let version = le32(&bytes, 4)?;
    if !matches!(version, 1 | 2) { return Err(invalid("only standard Valve VPK v1/v2 framing is understood")); }
    let tree_size = le32(&bytes, 8)?;
    if tree_size == 0 || tree_size > MAX_TREE { return Err(invalid("VPK directory exceeds the 32 MiB audit limit")); }
    let size = if version == 1 { 12u64 } else { 28u64 };
    let data_start = size.checked_add(tree_size as u64).ok_or_else(|| invalid("VPK data offset overflow"))?;
    source.range(size, tree_size as u64)?;
    let mut h = Header { version, size, tree_size, data_start,
        data_size: 0, archive_md5: 0, other_md5: 0, signature: 0, trailing: 0 };
    if version == 1 {
        h.data_size = source.len().checked_sub(data_start).ok_or_else(|| invalid("VPK tree exceeds source bounds"))?;
    } else {
        let extra = source.read_at(12, 16)?;
        h.data_size = le32(&extra, 0)? as u64;
        h.archive_md5 = le32(&extra, 4)?;
        h.other_md5 = le32(&extra, 8)?;
        h.signature = le32(&extra, 12)?;
        let end = data_start.checked_add(h.data_size)
            .and_then(|n| n.checked_add(h.archive_md5 as u64))
            .and_then(|n| n.checked_add(h.other_md5 as u64))
            .and_then(|n| n.checked_add(h.signature as u64))
            .ok_or_else(|| invalid("VPK section lengths overflow"))?;
        if end > source.len() { return Err(invalid("VPK declared sections exceed source bounds")); }
        // New signature variants may have additional bytes after a section stub.
        // Report the trailer as opaque, not as validated integrity metadata.
        h.trailing = source.len() - end;
    }
    Ok(h)
}

fn path_status(path: &[u8]) -> &'static str {
    if path.starts_with(b"/") || path.contains(&b'\\') || path.contains(&b':')
        || path.split(|b| *b == b'/').any(|p| p.is_empty() || p == b"." || p == b"..") {
        "UNSAFE_OR_NONSTANDARD_PATH"
    } else if !path.is_ascii() || path.iter().any(|b| !(0x20..=0x7e).contains(b)) {
        "NON_ASCII_OR_CONTROL_PATH_RULES_UNPROVEN"
    } else { "DECLARED_PATH_ONLY" }
}

fn inspect(source: &mut Source) -> io::Result<Inventory> {
    let h = header(source)?;
    let bytes = source.read_at(h.size, h.tree_size as usize)?;
    let mut c = Cursor { bytes: &bytes, pos: 0 };
    let mut v = Inventory { header: h, entries: Vec::new(), extensions: BTreeMap::new(),
        archives: BTreeMap::new(), logical_bytes: 0 };
    let mut paths = BTreeSet::new();
    let mut name_budget = MAX_RETAINED_NAMES;
    loop {
        let extension = c.text()?;
        if extension.is_empty() { break; }
        loop {
            let directory = c.text()?;
            if directory.is_empty() { break; }
            loop {
                super::cancelled()?;
                let name = c.text()?;
                if name.is_empty() { break; }
                if v.entries.len() >= MAX_ENTRIES { return Err(invalid("VPK entry count exceeds audit limit")); }
                let crc = c.u32()?;
                let preload_size = c.u16()?;
                let archive = c.u16()?;
                let offset = c.u32()?;
                let length = c.u32()?;
                if c.u16()? != 0xffff { return Err(invalid("invalid VPK entry terminator")); }
                if archive > 0x7fff { return Err(invalid("unsupported VPK archive index")); }
                let preload_offset = v.header.size.checked_add(c.pos as u64)
                    .ok_or_else(|| invalid("VPK preload offset overflow"))?;
                c.take(preload_size as usize)?; // Bytes belong to the entry, not a separate archive.
                let mut path = Vec::new();
                if directory != b" " { path.extend_from_slice(&directory); path.push(b'/'); }
                path.extend_from_slice(&name);
                if extension != b" " { path.push(b'.'); path.extend_from_slice(&extension); }
                if path.len() > 8192 { return Err(invalid("VPK combined path exceeds audit limit")); }
                // Repeated long directory names can amplify a small tree into
                // much larger retained entry paths. Bound that separately from
                // the raw directory-byte and entry-count limits.
                name_budget = name_budget.checked_sub(path.len() + extension.len())
                    .ok_or_else(|| super::audit_io::budget_error("VPK retained path-name budget exceeded"))?;
                let status = path_status(&path);
                let folded: Vec<u8> = path.iter().map(u8::to_ascii_lowercase).collect();
                if !paths.insert(folded) { return Err(invalid("ambiguous duplicate/case-folded VPK entry path")); }
                let total = (preload_size as u64).checked_add(length as u64)
                    .ok_or_else(|| invalid("VPK entry length overflow"))?;
                v.logical_bytes = v.logical_bytes.checked_add(total).ok_or_else(|| invalid("VPK logical-byte sum overflow"))?;
                let extent_end = (offset as u64).checked_add(length as u64)
                    .ok_or_else(|| invalid("VPK archive extent overflow"))?;
                if length == 0 {
                    // Preload-only entries need no archive extent. Their unused
                    // offset/archive fields are reported, not dereferenced.
                } else if archive == 0x7fff {
                    if extent_end > v.header.data_size { return Err(invalid("VPK embedded entry exceeds the file-data section")); }
                } else {
                    let required = v.archives.entry(archive).or_default();
                    *required = (*required).max(extent_end);
                }
                let slot = v.extensions.entry(extension.iter().map(u8::to_ascii_lowercase).collect()).or_default();
                slot.0 += 1; slot.1 = slot.1.checked_add(total).ok_or_else(|| invalid("VPK extension-byte sum overflow"))?;
                v.entries.push(Entry { path, crc, preload_offset, preload_size, archive, offset, length, path_status: status });
            }
        }
    }
    if c.pos != bytes.len() { return Err(invalid("unparsed bytes in the declared VPK directory tree")); }
    Ok(v)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, 8 * 1024 * 1024 * 1024, MAX_TREE as u64 + 64)?;
    let v = inspect(&mut source)?;
    source.unchanged()?;
    let h = &v.header;
    println!("VPK_HEADER|{}|{}|{}|{}|{}|{}|{}|{}|{}", h.version, h.size, h.tree_size,
        h.data_start, h.data_size, h.archive_md5, h.other_md5, h.signature, h.trailing);
    for (extension, (count, bytes)) in &v.extensions {
        println!("VPK_ENTRY_KIND|{}|{count}|{bytes}", field(extension));
    }
    for entry in v.entries.iter().take(MAX_REPORTS) {
        println!("VPK_ENTRY|{}|{}|{}|{}|{}|{}|{}|{}", field(&entry.path), entry.crc,
            entry.preload_offset, entry.preload_size, entry.archive, entry.offset, entry.length, entry.path_status);
    }
    for (archive, length) in &v.archives {
        println!("VPK_ARCHIVE_REQUIRED|{archive}|{length}|COMPANION_NOT_OPENED");
    }
    println!("VPK_TOTAL|{}|{}|{}|METADATA_ONLY", v.entries.len(), v.entries.len().min(MAX_REPORTS), v.logical_bytes);
    println!("VPK_INTEGRITY|UNVERIFIED|CRC_MD5_SIGNATURES_NOT_CHECKED");
    eprintln!("Development read-only Valve VPK directory audit; logical bytes include entry preload segments. Numbered companions and entry payloads are not opened, signatures/checksums stay unverified, and no Source texture writer or archive repacker exists.");
    Ok(())
}
