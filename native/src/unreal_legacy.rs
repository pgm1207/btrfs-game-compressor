//! Read-only legacy Pak v1–9 directory/header checks. No decompression, export,
//! path extraction or cooked asset rewrite. Modern v10/v11 indexes are separate.
//! Layout reference: raum/repak `src/entry.rs` (documentation only, no dependency).
use std::{collections::{BTreeMap, BTreeSet}, io::{self, Read, Seek, SeekFrom}};
use sha1::{Digest, Sha1};
fn bad(s: &str) -> io::Error { super::invalid(s) }

pub(crate) struct Cursor<'a> { b: &'a [u8], pos: usize }
impl<'a> Cursor<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Self { Self { b, pos: 0 } }
    pub(crate) fn pos(&self) -> usize { self.pos }
    pub(crate) fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("Pak index offset overflow"))?;
        let b = self.b.get(self.pos..end).ok_or_else(|| bad("truncated Pak index"))?;
        self.pos = end; Ok(b)
    }
    pub(crate) fn u8(&mut self) -> io::Result<u8> { Ok(self.take(1)?[0]) }
    pub(crate) fn u32(&mut self) -> io::Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    pub(crate) fn i32(&mut self) -> io::Result<i32> { Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    pub(crate) fn u64(&mut self) -> io::Result<u64> { Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())) }
    pub(crate) fn string(&mut self) -> io::Result<String> {
        let size = self.i32()?;
        let count = size.checked_abs().ok_or_else(|| bad("invalid Pak string size"))? as usize;
        if count == 0 || count > 4096 { return Err(bad("Pak string exceeds bounds")); }
        let text = if size > 0 {
            let b = self.take(count)?;
            if b[count - 1] != 0 { return Err(bad("Pak string is not terminated")); }
            std::str::from_utf8(&b[..count - 1]).map_err(|_| bad("invalid Pak UTF-8"))?.to_owned()
        } else {
            let b = self.take(count * 2)?;
            let values: Vec<u16> = b.chunks_exact(2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).collect();
            if values[count - 1] != 0 { return Err(bad("Pak UTF-16 is not terminated")); }
            String::from_utf16(&values[..count - 1]).map_err(|_| bad("invalid Pak UTF-16"))?
        };
        if text.chars().any(|c| c.is_control() || c == '|') { return Err(bad("unsafe Pak index text")); }
        Ok(text)
    }
}
pub(crate) struct Entry<'a> {
    pub offset: u64, pub stored: u64, pub decoded: u64, pub method: u32, pub flags: u8,
    pub hash: [u8; 20], pub raw: &'a [u8],
}
/// Non-encoded Pak entry, used by v1–9 indexes and modern secondary tables.
pub(crate) fn entry<'a>(c: &mut Cursor<'a>, version: u32, index_offset: u64, compression_u8: bool) -> io::Result<Entry<'a>> {
    let start = c.pos();
    let offset = c.u64()?; let stored = c.u64()?; let decoded = c.u64()?;
    let method = if version >= 8 && compression_u8 { u32::from(c.u8()?) } else { c.u32()? };
    if version == 1 { c.u64()?; }
    let hash = c.take(20)?.try_into().unwrap();
    let mut blocks = Vec::new();
    if version >= 3 && method != 0 {
        let count = c.u32()? as usize;
        if count == 0 || count > 65536 { return Err(bad("invalid Pak compression block count")); }
        for _ in 0..count { blocks.push((c.u64()?, c.u64()?)); }
    }
    let flags = if version >= 3 { c.u8()? } else { 0 };
    if flags & !3 != 0 { return Err(bad("unsupported Pak entry flags")); }
    if version >= 3 { c.u32()?; }
    let raw = &c.b[start..c.pos()];
    if flags & 2 == 0 {
        let base = offset.checked_add(raw.len() as u64).ok_or_else(|| bad("Pak entry offset overflow"))?;
        let physical = if flags & 1 != 0 { stored.checked_add(15).ok_or_else(|| bad("Pak encrypted size overflow"))? / 16 * 16 } else { stored };
        let end = base.checked_add(physical).ok_or_else(|| bad("Pak entry end overflow"))?;
        if end > index_offset || method == 0 && stored != decoded { return Err(bad("Pak entry exceeds data bounds or has inconsistent sizes")); }
        // v1–4 block offsets are absolute; v5+ are entry-relative and already
        // validated against the entry extent, so only bounds are enforced there.
        let mut previous = 0u64;
        for (s, e) in blocks {
            let s = if version >= 5 { offset.checked_add(s).ok_or_else(|| bad("Pak block offset overflow"))? } else { s };
            let e = if version >= 5 { offset.checked_add(e).ok_or_else(|| bad("Pak block end overflow"))? } else { e };
            if e <= s || e > end || version < 5 && s < previous { return Err(bad("invalid Pak block ranges")); }
            previous = e;
        }
    }
    Ok(Entry { offset, stored, decoded, method, flags, hash, raw })
}

#[derive(Debug, Default)]
pub struct Report {
    pub entries: u64, pub stored: u64, pub encrypted: u64, pub deleted: u64,
    pub header_checked: u64, pub payload_checked: u64, pub payload_skipped: u64,
    pub methods: BTreeMap<String, u64>, pub kinds: BTreeMap<&'static str, u64>,
}
pub(crate) fn method_name(version: u32, method: u32, codecs: &[String]) -> String {
    if method == 0 { return "none".into(); }
    if version < 8 { return method.to_string(); }
    codecs.get((method - 1) as usize).filter(|s| !s.is_empty())
        .cloned().unwrap_or_else(|| format!("slot{method}"))
}
pub fn kind_of(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
        Some("uasset") => "uasset", Some("uexp") => "uexp", Some("ubulk") => "ubulk",
        Some("uptnl") => "uptnl",
        Some("wem") | Some("bnk") => "wwise-audio",
        Some("png") | Some("jpg") | Some("svg") => "raw-image",
        Some("uplugin") | Some("ini") | Some("locres") | Some("locmeta") | Some("csv") => "config",
        _ => "other",
    }
}
pub fn inspect<R: Read + Seek>(file: &mut R, version: u32, index_offset: u64, bytes: &[u8],
    codecs: &[String], compression_u8: bool) -> io::Result<Report> {
    if !(1..=9).contains(&version) { return Err(bad("legacy Pak audit requires v1–9")); }
    let mut c = Cursor::new(bytes); c.string()?; // mount point, never followed
    let count = c.u32()? as usize;
    if count > 2_000_000 { return Err(bad("Pak entry count exceeds limit")); }
    let mut report = Report::default(); let mut names = BTreeSet::new(); let mut extents = Vec::new();
    let mut budget = 64u64 * 1024 * 1024; let mut buffer = [0; 65536];
    for _ in 0..count {
        super::cancelled()?;
        let name = c.string()?;
        if name.is_empty() || !names.insert(name.clone()) { return Err(bad("empty/duplicate Pak filename")); }
        let e = entry(&mut c, version, index_offset, compression_u8)?;
        report.entries += 1; *report.methods.entry(method_name(version, e.method, codecs)).or_default() += 1;
        *report.kinds.entry(kind_of(&name)).or_default() += 1;
        if e.flags & 2 != 0 { report.deleted += 1; continue; }
        let physical = if e.flags & 1 != 0 { e.stored.div_ceil(16) * 16 } else { e.stored };
        extents.push((e.offset, e.offset + e.raw.len() as u64 + physical));
        file.seek(SeekFrom::Start(e.offset))?;
        let mut header = vec![0; e.raw.len()]; file.read_exact(&mut header)?;
        // Data headers have a zero offset; remaining indexed fields must match.
        if u64::from_le_bytes(header[..8].try_into().unwrap()) != 0 || header[8..] != e.raw[8..] {
            return Err(bad("Pak data header differs from indexed entry"));
        }
        report.header_checked += 1;
        if e.flags & 1 != 0 { report.encrypted += 1; report.payload_skipped += 1; continue; }
        if e.method != 0 { report.payload_skipped += 1; continue; }
        report.stored += 1;
        if e.stored > budget { report.payload_skipped += 1; continue; }
        budget -= e.stored; let mut remaining = e.stored; let mut hash = Sha1::new();
        while remaining > 0 {
            super::cancelled()?; let n = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n])?; hash.update(&buffer[..n]); remaining -= n as u64;
        }
        let actual: [u8; 20] = hash.finalize().into();
        if actual != e.hash { return Err(bad("Pak stored payload SHA1 mismatch")); }
        report.payload_checked += 1;
    }
    extents.sort_unstable();
    if extents.windows(2).any(|p| p[0].1 > p[1].0) { return Err(bad("overlapping Pak entry extents")); }
    if c.pos() != bytes.len() { return Err(bad("unknown legacy Pak index trailer")); }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn string(b: &mut Vec<u8>, text: &str) { b.extend(((text.len() + 1) as u32).to_le_bytes()); b.extend(text.as_bytes()); b.push(0); }
    fn record(version: u32, method: u32, flags: u8, stored: u64, decoded: u64, hash: &[u8], blocks: &[(u64, u64)]) -> Vec<u8> {
        let mut r = Vec::new(); r.extend(0u64.to_le_bytes());
        r.extend(stored.to_le_bytes()); r.extend(decoded.to_le_bytes());
        if version >= 8 { r.extend(method.to_le_bytes()); } else { r.extend(method.to_le_bytes()); }
        if version == 1 { r.extend(0u64.to_le_bytes()); }
        r.extend(hash);
        if version >= 3 && method != 0 {
            r.extend((blocks.len() as u32).to_le_bytes());
            for (s, e) in blocks { r.extend(s.to_le_bytes()); r.extend(e.to_le_bytes()); }
        }
        if version >= 3 { r.push(flags); r.extend(0u32.to_le_bytes()); }
        r
    }
    fn fixture(version: u32, method: u32, flags: u8, payload: &[u8], decoded: u64, blocks: &[(u64, u64)]) -> (Vec<u8>, u64, Vec<u8>) {
        let rec = record(version, method, flags, payload.len() as u64, decoded, &Sha1::digest(payload), blocks);
        let mut file = rec.clone(); file.extend(payload); let index_offset = file.len() as u64;
        // Encrypted payloads occupy 16-byte aligned storage.
        if flags & 1 != 0 { while file.len() % 16 != 0 { file.push(0); } let index_offset = file.len() as u64; let mut index = Vec::new();
            string(&mut index, "../../../Project/Content/"); index.extend(1u32.to_le_bytes());
            string(&mut index, "Textures/example.uasset"); index.extend(&rec); file.extend(&index);
            return (file, index_offset, index);
        }
        let mut index = Vec::new(); string(&mut index, "../../../Project/Content/");
        index.extend(1u32.to_le_bytes()); string(&mut index, "Textures/example.uasset"); index.extend(&rec);
        file.extend(&index); (file, index_offset, index)
    }
    #[test]
    fn legacy_versions_validate_headers_and_stored_hashes_without_extraction() {
        for version in 1..=9 {
            let (file, offset, index) = fixture(version, 0, 0, b"abc", 3, &[]);
            let r = inspect(&mut io::Cursor::new(&file), version, offset, &index, &[], false).unwrap();
            assert_eq!((r.entries, r.header_checked, r.payload_checked, r.payload_skipped), (1, 1, 1, 0));
            assert_eq!(r.methods["none"], 1); assert_eq!(r.kinds["uasset"], 1);
            for short in 0..index.len() { assert!(inspect(&mut io::Cursor::new(&file), version, offset, &index[..short], &[], false).is_err()); }
            let mut broken = file.clone(); broken[offset as usize - 1] ^= 1;
            assert!(inspect(&mut io::Cursor::new(broken), version, offset, &index, &[], false).unwrap_err().to_string().contains("SHA1"));
            let mut broken = file.clone(); broken[8] ^= 1;
            assert!(inspect(&mut io::Cursor::new(broken), version, offset, &index, &[], false).unwrap_err().to_string().contains("data header"));
        }
    }
    #[test]
    fn v8_compression_slots_map_to_footer_names_and_reject_future_versions() {
        let mut payload = b"abc".to_vec(); payload.resize(20, 0);
        let (file, offset, index) = fixture(8, 1, 0, &payload, 20, &[(0, 3)]);
        let codecs = vec!["Oodle".to_owned(), String::new(), String::new(), String::new(), String::new()];
        let r = inspect(&mut io::Cursor::new(&file), 8, offset, &index, &codecs, false).unwrap();
        assert_eq!(r.methods["Oodle"], 1); assert_eq!((r.payload_checked, r.payload_skipped), (0, 1));
        assert!(inspect(&mut io::Cursor::new(&file), 10, offset, &index, &codecs, false).is_err());
        assert!(inspect(&mut io::Cursor::new(&file), 8, offset, &index, &codecs, true).is_err());
    }
    #[test]
    fn rejects_oversized_indexes_and_unknown_entry_flags() {
        let (file, offset, _) = fixture(7, 0, 0, b"abc", 3, &[]);
        let mut index = Vec::new(); string(&mut index, "/"); index.extend(1u32.to_le_bytes());
        string(&mut index, "a.uasset");
        index.extend(record(7, 0, 0x80, 3, 3, &Sha1::digest(b"abc"), &[]));
        assert!(inspect(&mut io::Cursor::new(&file), 7, offset, &index, &[], false).is_err());
    }
    #[test]
    fn utf16_and_string_lengths_are_bounded() {
        let mut b = (-3i32).to_le_bytes().to_vec(); for v in [b'A' as u16, b'B' as u16, 0] { b.extend(v.to_le_bytes()); }
        assert_eq!(Cursor::new(&b).string().unwrap(), "AB");
        for n in 0..b.len() { assert!(Cursor::new(&b[..n]).string().is_err()); }
        assert!(Cursor::new(&i32::MIN.to_le_bytes()).string().is_err());
    }
    #[test]
    fn encrypted_entries_are_reported_without_attempting_to_hash_or_decrypt() {
        let payload = [0u8; 16];
        let (file, offset, index) = fixture(7, 0, 1, &payload, 16, &[]);
        let r = inspect(&mut io::Cursor::new(&file), 7, offset, &index, &[], false).unwrap();
        assert_eq!((r.encrypted, r.header_checked, r.payload_checked, r.payload_skipped), (1, 1, 0, 1));
    }
    #[test]
    fn compressed_block_extents_are_checked_but_payloads_are_not_called_verified() {
        let mut payload = b"not".to_vec(); payload.resize(20, 0);
        let (file, offset, index) = fixture(7, 1, 0, &payload, 8, &[(0, 3)]);
        let r = inspect(&mut io::Cursor::new(&file), 7, offset, &index, &[], false).unwrap();
        assert_eq!((r.header_checked, r.payload_checked, r.payload_skipped), (1, 0, 1));
        assert_eq!(r.methods["1"], 1); assert_eq!(r.kinds["uasset"], 1);
        // A block beyond the declared entry extent must fail closed.
        let bad = record(7, 1, 0, 3, 8, &Sha1::digest(b"not"), &[(0, 9999)]);
        let mut index2 = Vec::new(); string(&mut index2, "/"); index2.extend(1u32.to_le_bytes()); string(&mut index2, "a.ubulk"); index2.extend(&bad);
        assert!(inspect(&mut io::Cursor::new(&file), 7, offset, &index2, &[], false).is_err());
    }
}
