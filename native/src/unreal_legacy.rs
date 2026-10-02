//! Read-only legacy Pak v1–7 directory/header checks. No decompression, export,
//! path extraction or cooked asset rewrite. Modern/frozen indexes are separate.
//! Layout reference: repak/src/entry.rs (format reference, no runtime dependency).
use std::{collections::{BTreeMap, BTreeSet}, io::{self, Read, Seek, SeekFrom}};
use sha1::{Digest, Sha1};
fn bad(s: &str) -> io::Error { super::invalid(s) }
struct Cursor<'a> { b: &'a [u8], pos: usize }
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("Pak index offset overflow"))?;
        let b = self.b.get(self.pos..end).ok_or_else(|| bad("truncated Pak index"))?;
        self.pos = end; Ok(b)
    }
    fn u32(&mut self) -> io::Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> io::Result<u64> { Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())) }
    fn string(&mut self) -> io::Result<String> {
        let size = self.u32()? as i32;
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
struct Entry<'a> { offset: u64, stored: u64, method: u32, flags: u8, hash: [u8; 20], raw: &'a [u8] }
fn entry<'a>(c: &mut Cursor<'a>, version: u32, index_offset: u64) -> io::Result<Entry<'a>> {
    let start = c.pos;
    let offset = c.u64()?; let stored = c.u64()?; let decoded = c.u64()?; let method = c.u32()?;
    if version == 1 { c.u64()?; }
    let hash = c.take(20)?.try_into().unwrap();
    let mut blocks = Vec::new();
    if version >= 3 && method != 0 {
        let count = c.u32()? as usize;
        if count == 0 || count > 65536 { return Err(bad("invalid Pak compression block count")); }
        for _ in 0..count { blocks.push((c.u64()?, c.u64()?)); }
    }
    let flags = if version >= 3 { c.take(1)?[0] } else { 0 };
    if flags & !3 != 0 { return Err(bad("unsupported Pak entry flags")); }
    if version >= 3 { c.u32()?; }
    let raw = &c.b[start..c.pos];
    if flags & 2 == 0 {
        let base = offset.checked_add(raw.len() as u64).ok_or_else(|| bad("Pak entry offset overflow"))?;
        let physical = if flags & 1 != 0 { stored.checked_add(15).ok_or_else(|| bad("Pak encrypted size overflow"))? / 16 * 16 } else { stored };
        let end = base.checked_add(physical).ok_or_else(|| bad("Pak entry end overflow"))?;
        if end > index_offset || method == 0 && stored != decoded { return Err(bad("Pak entry exceeds data bounds or has inconsistent sizes")); }
        let mut previous = base;
        for (s, e) in blocks {
            let s = if version >= 5 { offset.checked_add(s).ok_or_else(|| bad("Pak block offset overflow"))? } else { s };
            let e = if version >= 5 { offset.checked_add(e).ok_or_else(|| bad("Pak block end overflow"))? } else { e };
            if s < previous || e <= s || e > end { return Err(bad("invalid Pak block ranges")); }
            previous = e;
        }
    }
    Ok(Entry { offset, stored, method, flags, hash, raw })
}

#[derive(Debug, Default)]
pub struct Report {
    pub entries: u64, pub stored: u64, pub encrypted: u64, pub deleted: u64,
    pub header_checked: u64, pub payload_checked: u64, pub payload_skipped: u64,
    pub methods: BTreeMap<u32, u64>, pub kinds: BTreeMap<&'static str, u64>,
}
pub fn inspect<R: Read + Seek>(file: &mut R, version: u32, offset: u64, size: u64) -> io::Result<Report> {
    if !(1..=7).contains(&version) || size > 32 * 1024 * 1024 { return Err(bad("legacy Pak audit requires v1–7 and an index <=32 MiB")); }
    let mut bytes = vec![0; size as usize]; file.seek(SeekFrom::Start(offset))?; file.read_exact(&mut bytes)?;
    let mut c = Cursor { b: &bytes, pos: 0 }; c.string()?; // mount point, never followed
    let count = c.u32()? as usize;
    if count > 200000 { return Err(bad("Pak entry count exceeds limit")); }
    let mut report = Report::default(); let mut names = BTreeSet::new(); let mut extents = Vec::new();
    let mut budget = 64u64 * 1024 * 1024; let mut buffer = [0; 65536];
    for _ in 0..count {
        super::cancelled()?;
        let name = c.string()?;
        if name.is_empty() || !names.insert(name.clone()) { return Err(bad("empty/duplicate Pak filename")); }
        let e = entry(&mut c, version, offset)?;
        report.entries += 1; *report.methods.entry(e.method).or_default() += 1;
        let kind = match name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
            Some("uasset") => "uasset", Some("uexp") => "uexp", Some("ubulk") => "ubulk", _ => "other",
        };
        *report.kinds.entry(kind).or_default() += 1;
        if e.flags & 2 != 0 { report.deleted += 1; continue; }
        let physical = if e.flags & 1 != 0 { e.stored.div_ceil(16) * 16 } else { e.stored };
        extents.push((e.offset, e.offset + e.raw.len() as u64 + physical));
        file.seek(SeekFrom::Start(e.offset))?;
        let mut header = vec![0; e.raw.len()]; file.read_exact(&mut header)?;
        // Data headers have a zero offset; all remaining indexed fields must
        // match exactly. Do not infer compatibility from a directory alone.
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
    if c.pos != bytes.len() { return Err(bad("unknown legacy Pak index trailer")); }
    extents.sort_unstable();
    if extents.windows(2).any(|p| p[0].1 > p[1].0) { return Err(bad("overlapping Pak entry extents")); }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn string(b: &mut Vec<u8>, text: &str) { b.extend(((text.len() + 1) as u32).to_le_bytes()); b.extend(text.as_bytes()); b.push(0); }
    fn fixture(version: u32) -> (Vec<u8>, u64, u64) {
        let mut h = Vec::new(); h.extend(0u64.to_le_bytes());
        h.extend(3u64.to_le_bytes()); h.extend(3u64.to_le_bytes()); h.extend(0u32.to_le_bytes());
        if version == 1 { h.extend(0u64.to_le_bytes()); }
        h.extend(Sha1::digest(b"abc"));
        if version >= 3 { h.push(0); h.extend(0u32.to_le_bytes()); }
        let mut bytes = h.clone(); bytes.extend(b"abc"); let offset = bytes.len() as u64;
        let mut index = Vec::new(); string(&mut index, "../../../Project/Content/");
        index.extend(1u32.to_le_bytes()); string(&mut index, "Textures/example.uasset"); index.extend(h);
        let size = index.len() as u64; bytes.extend(index); (bytes, offset, size)
    }
    #[test]
    fn legacy_versions_validate_headers_and_stored_hashes_without_extraction() {
        for version in 1..=7 {
            let (bytes, offset, size) = fixture(version);
            let r = inspect(&mut io::Cursor::new(&bytes), version, offset, size).unwrap();
            assert_eq!((r.entries, r.header_checked, r.payload_checked, r.payload_skipped), (1, 1, 1, 0));
            assert_eq!(r.methods[&0], 1); assert_eq!(r.kinds["uasset"], 1);
            for short in 0..size { assert!(inspect(&mut io::Cursor::new(&bytes), version, offset, short).is_err()); }
            let mut broken = bytes.clone(); broken[offset as usize - 1] ^= 1;
            assert!(inspect(&mut io::Cursor::new(broken), version, offset, size).unwrap_err().to_string().contains("SHA1"));
            let mut broken = bytes.clone(); broken[8] ^= 1;
            assert!(inspect(&mut io::Cursor::new(broken), version, offset, size).unwrap_err().to_string().contains("data header"));
        }
    }
    #[test]
    fn rejects_future_versions_oversized_indexes_and_unknown_entry_flags() {
        let (bytes, offset, size) = fixture(7);
        assert!(inspect(&mut io::Cursor::new(&bytes), 8, offset, size).is_err());
        assert!(inspect(&mut io::Cursor::new(&bytes), 7, offset, 32 * 1024 * 1024 + 1).is_err());
        let mut broken = bytes; let flag = broken.len() - 5; broken[flag] = 0x80;
        assert!(inspect(&mut io::Cursor::new(broken), 7, offset, size).is_err());
    }
    #[test]
    fn utf16_and_string_lengths_are_bounded() {
        let mut b = (-3i32).to_le_bytes().to_vec(); for v in [b'A' as u16, b'B' as u16, 0] { b.extend(v.to_le_bytes()); }
        assert_eq!(Cursor { b: &b, pos: 0 }.string().unwrap(), "AB");
        for n in 0..b.len() { assert!(Cursor { b: &b[..n], pos: 0 }.string().is_err()); }
        assert!(Cursor { b: &i32::MIN.to_le_bytes(), pos: 0 }.string().is_err());
    }
    #[test]
    fn encrypted_entries_are_reported_without_attempting_to_hash_or_decrypt() {
        let (mut bytes, offset, size) = fixture(7);
        bytes[48] = 1; let flag = bytes.len() - 5; bytes[flag] = 1;
        bytes.splice(offset as usize..offset as usize, [0; 13]); // AES-padded storage, no decryption
        let r = inspect(&mut io::Cursor::new(bytes), 7, offset + 13, size).unwrap();
        assert_eq!((r.encrypted, r.header_checked, r.payload_checked, r.payload_skipped), (1, 1, 0, 1));
    }
    #[test]
    fn compressed_block_extents_are_checked_but_payloads_are_not_called_verified() {
        let mut h = Vec::new(); h.extend(0u64.to_le_bytes()); h.extend(3u64.to_le_bytes());
        h.extend(8u64.to_le_bytes()); h.extend(1u32.to_le_bytes()); h.extend([0; 20]);
        h.extend(1u32.to_le_bytes()); h.extend(73u64.to_le_bytes()); h.extend(76u64.to_le_bytes());
        h.push(0); h.extend(8u32.to_le_bytes()); assert_eq!(h.len(), 73);
        let mut b = h.clone(); b.extend(b"not"); let offset = b.len() as u64;
        let mut index = Vec::new(); string(&mut index, "/"); index.extend(1u32.to_le_bytes());
        string(&mut index, "texture.ubulk"); index.extend(h); let size = index.len() as u64;
        b.extend(index); let r = inspect(&mut io::Cursor::new(&b), 7, offset, size).unwrap();
        assert_eq!((r.header_checked, r.payload_checked, r.payload_skipped), (1, 0, 1));
        assert_eq!(r.methods[&1], 1); assert_eq!(r.kinds["ubulk"], 1);
        let block_end = b.len() - 13; b[block_end..block_end + 8].copy_from_slice(&77u64.to_le_bytes());
        assert!(inspect(&mut io::Cursor::new(b), 7, offset, size).is_err());
    }
}
