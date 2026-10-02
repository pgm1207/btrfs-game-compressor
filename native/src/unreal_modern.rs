//! Read-only modern Pak v10/v11 index audit. Primary and secondary index hashes
//! are verified before parsing; individual encoded entries are bounded-decoded
//! only for classification. No decompression, path resolution or repacking.
//! Layout reference: raum/repak `src/pak.rs` (documentation only, no dependency).
use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use sha1::{Digest, Sha1};
use super::unreal_legacy::{entry, kind_of, method_name, Cursor, Entry};
fn bad(s: &str) -> io::Error { super::invalid(s) }

const MAX_SECONDARY: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: u64 = 2_000_000;

struct Encoded { method: Option<u32>, encrypted: bool, compressed: u64, decoded: u64, offset: u64 }
fn decode_encoded(c: &mut Cursor<'_>) -> io::Result<Encoded> {
    let bits = c.u32()?;
    let method = match (bits >> 23) & 0x3f { 0 => None, n => Some(n - 1) };
    let encrypted = bits & (1 << 22) != 0;
    let blocks = (bits >> 6) & 0xffff;
    let block_size = bits & 0x3f;
    if block_size == 0x3f { c.u32()?; }
    let bit = |b: u32| bits & (1 << b) != 0;
    let var = |c: &mut Cursor<'_>, b: u32| -> io::Result<u64> {
        if bit(b) { Ok(c.u32()? as u64) } else { c.u64() }
    };
    let offset = var(c, 31)?;
    let decoded = var(c, 30)?;
    let compressed = match method { None => decoded, Some(_) => var(c, 29)? };
    if method.is_some() && blocks == 0 { return Err(bad("compressed Pak entry has no blocks")); }
    if method.is_none() && blocks != 0 { return Err(bad("stored Pak entry declares blocks")); }
    if blocks > 1 || encrypted { for _ in 0..blocks { c.u32()?; } }
    Ok(Encoded { method, encrypted, compressed, decoded, offset })
}
fn check_encoded(e: &Encoded, index_offset: u64) -> io::Result<()> {
    if !e.encrypted && e.offset.checked_add(e.compressed).is_none_or(|end| end > index_offset) {
        return Err(bad("Pak encoded entry exceeds data bounds"));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct Report {
    pub entry_count: u64, pub files: u64, pub path_hash_seed: u64,
    pub has_path_hash_index: bool, pub has_directory_index: bool,
    pub encoded_bytes: u64, pub non_encoded: u64,
    pub secondary: Vec<(&'static str, &'static str, u64)>,
    pub methods: BTreeMap<String, u64>, pub kinds: BTreeMap<&'static str, u64>,
    pub encrypted: u64, pub compressed: u64, pub deleted: u64,
    pub stored_bytes: u64, pub decoded_bytes: u64,
}
struct Secondary { offset: u64, size: u64, hash: [u8; 20] }
fn read_secondary<R: Read + Seek>(file: &mut R, s: &Secondary) -> io::Result<&'static str> {
    if s.size == 0 || s.size > MAX_SECONDARY { return Ok("SKIPPED_BUDGET"); }
    file.seek(SeekFrom::Start(s.offset))?;
    let mut bytes = vec![0; s.size as usize]; file.read_exact(&mut bytes)?;
    let mut hash = Sha1::new(); hash.update(&bytes);
    if <[u8; 20]>::from(hash.finalize()) != s.hash { return Err(bad("Unreal Pak secondary index SHA1 mismatch")); }
    Ok("VERIFIED")
}

pub fn inspect<R: Read + Seek>(file: &mut R, version: u32, index_offset: u64, data_limit: u64, bytes: &[u8], codecs: &[String]) -> io::Result<Report> {
    if !(10..=11).contains(&version) { return Err(bad("modern Pak audit requires v10/v11")); }
    let mut c = Cursor::new(bytes); c.string()?; // mount point, never followed
    let entry_count = c.u32()? as u64;
    if entry_count > MAX_ENTRIES { return Err(bad("Pak entry count exceeds limit")); }
    let mut report = Report { entry_count, ..Report::default() };
    report.path_hash_seed = c.u64()?;
    let read_secondary_header = |c: &mut Cursor<'_>| -> io::Result<Option<Secondary>> {
        if c.u32()? == 0 { return Ok(None); }
        let s = Secondary { offset: c.u64()?, size: c.u64()?, hash: c.take(20)?.try_into().unwrap() };
        if s.size == 0 || s.offset.checked_add(s.size).is_none_or(|e| e > data_limit) { return Err(bad("Pak secondary index exceeds data bounds")); }
        Ok(Some(s))
    };
    let phi = read_secondary_header(&mut c)?;
    let fdi = read_secondary_header(&mut c)?;
    report.has_path_hash_index = phi.is_some(); report.has_directory_index = fdi.is_some();
    for (name, s) in [("path_hash_index", &phi), ("directory_index", &fdi)] {
        if let Some(s) = s { report.secondary.push((name, read_secondary(file, s)?, s.size)); }
    }
    let size = c.u32()? as usize;
    let encoded = c.take(size)?; report.encoded_bytes = size as u64;
    let non_encoded = c.u32()? as u64;
    if non_encoded > MAX_ENTRIES { return Err(bad("Pak non-encoded entry count exceeds limit")); }
    report.non_encoded = non_encoded;
    let mut plain: Vec<Entry> = Vec::with_capacity(non_encoded.min(65536) as usize);
    for _ in 0..non_encoded { plain.push(entry(&mut c, version, index_offset, false)?); }
    if c.pos() != bytes.len() { return Err(bad("unknown modern Pak index trailer")); }

    let tally = |e: &Encoded, name: Option<&str>, report: &mut Report| {
        let method = e.method.map(|m| method_name(version, m + 1, codecs)).unwrap_or_else(|| "none".into());
        *report.methods.entry(method).or_default() += 1;
        if e.encrypted { report.encrypted += 1; } else { report.stored_bytes += e.compressed; }
        report.decoded_bytes += e.decoded;
        if e.method.is_some() { report.compressed += 1; }
        if let Some(name) = name { *report.kinds.entry(kind_of(name)).or_default() += 1; }
    };
    let mut work = MAX_ENTRIES;
    if let Some(fdi) = fdi {
        // Re-read the verified directory index from the file for a second pass.
        file.seek(SeekFrom::Start(fdi.offset))?;
        let mut dirs = vec![0; fdi.size as usize]; file.read_exact(&mut dirs)?;
        let mut d = Cursor::new(&dirs);
        let dir_count = d.u32()? as usize;
        for _ in 0..dir_count {
            super::cancelled()?;
            let _dir = d.string()?;
            let files = d.u32()? as usize;
            report.files += files as u64;
            for _ in 0..files {
                work = work.checked_sub(1).ok_or_else(|| bad("Pak entry work budget exceeded"))?;
                let name = d.string()?;
                let at = d.i32()?;
                if at == i32::MIN { report.deleted += 1; continue; }
                let e = if at >= 0 {
                    let mut ec = Cursor::new(encoded); ec.take(at as usize)?;
                    decode_encoded(&mut ec)?
                } else {
                    plain.get((-at) as usize - 1).map(|p| Encoded {
                        method: (p.method != 0).then_some(p.method - 1), encrypted: p.flags & 1 != 0,
                        compressed: p.stored, decoded: p.decoded, offset: p.offset,
                    }).ok_or_else(|| bad("Pak directory index references a missing entry"))?
                };
                check_encoded(&e, index_offset)?;
                tally(&e, Some(&name), &mut report);
            }
        }
        if d.pos() != dirs.len() { return Err(bad("unknown directory index trailer")); }
    } else {
        // No directory index: encoded records are contiguous in entry order.
        let mut ec = Cursor::new(encoded);
        for _ in 0..entry_count {
            work = work.checked_sub(1).ok_or_else(|| bad("Pak entry work budget exceeded"))?;
            let e = decode_encoded(&mut ec)?; check_encoded(&e, index_offset)?; tally(&e, None, &mut report);
        }
        report.files = entry_count;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn string(b: &mut Vec<u8>, s: &str) { b.extend(((s.len() + 1) as u32).to_le_bytes()); b.extend(s.as_bytes()); b.push(0); }
    fn sha(b: &[u8]) -> [u8; 20] { Sha1::digest(b).into() }
    fn encode_entry(method: Option<u32>, encrypted: bool, data: &[u8]) -> Vec<u8> {
        let blocks = u32::from(method.is_some());
        let bits = (method.map_or(0, |m| m + 1) << 23) | (u32::from(encrypted) << 22) | (blocks << 6)
            | 1 << 31 | 1 << 30 | (u32::from(method.is_some()) << 29);
        let mut b = bits.to_le_bytes().to_vec();
        b.extend(0u32.to_le_bytes()); // offset (32-bit slot)
        b.extend((data.len() as u32).to_le_bytes()); // decoded (32-bit slot)
        if method.is_some() { b.extend((data.len() as u32).to_le_bytes()); }
        if blocks > 1 || encrypted { b.extend((data.len() as u32).to_le_bytes()); }
        b
    }
    /// Returns (file, index_offset, main_index), plus the FDI start and PHI start.
    fn fixture(method: Option<u32>, encoded_off: i32) -> (Vec<u8>, u64, Vec<u8>, u64, u64) {
        let encoded = if encoded_off == i32::MIN { Vec::new() } else { encode_entry(method, false, b"payload") };
        let mut fdi = Vec::new(); fdi.extend(1u32.to_le_bytes()); string(&mut fdi, "Textures/");
        fdi.extend(1u32.to_le_bytes()); string(&mut fdi, "a.uasset"); fdi.extend(encoded_off.to_le_bytes());
        let mut phi = Vec::new(); phi.extend(1u32.to_le_bytes()); phi.extend(0u64.to_le_bytes()); phi.extend(0u32.to_le_bytes()); phi.extend(0u32.to_le_bytes());
        let mut file = vec![0u8; 16]; // data area; the encoded record's declared offset points inside it
        let fdi_off = file.len() as u64; file.extend(&fdi);
        let phi_off = file.len() as u64; file.extend(&phi);
        let index_off = file.len() as u64;
        let mut index = Vec::new(); string(&mut index, "../../../");
        index.extend(1u32.to_le_bytes()); index.extend(7u64.to_le_bytes());
        index.extend(1u32.to_le_bytes()); index.extend(phi_off.to_le_bytes()); index.extend((phi.len() as u64).to_le_bytes()); index.extend(sha(&phi));
        index.extend(1u32.to_le_bytes()); index.extend(fdi_off.to_le_bytes()); index.extend((fdi.len() as u64).to_le_bytes()); index.extend(sha(&fdi));
        index.extend((encoded.len() as u32).to_le_bytes()); index.extend(&encoded);
        index.extend(0u32.to_le_bytes());
        file.extend(&index);
        (file, index_off, index, fdi_off, phi_off)
    }
    const CODECS: [&str; 5] = ["Zlib", "Gzip", "Oodle", "Zstd", "LZ4"];
    #[test]
    fn modern_index_verifies_secondary_hashes_and_classifies_entries() {
        let (file, offset, index, _, _) = fixture(None, 0);
        let r = inspect(&mut io::Cursor::new(&file), 11, offset, offset, &index, &[]).unwrap();
        assert_eq!((r.entry_count, r.files, r.path_hash_seed), (1, 1, 7));
        assert!(r.has_path_hash_index && r.has_directory_index);
        assert_eq!(r.encrypted, 0); assert_eq!(r.methods["none"], 1); assert_eq!(r.kinds["uasset"], 1);
        assert_eq!(r.secondary.iter().map(|s| s.1).collect::<Vec<_>>(), ["VERIFIED", "VERIFIED"]);
    }
    #[test]
    fn named_compression_is_classified_without_decoding_and_bad_secondary_fails() {
        let (file, offset, index, fdi_off, _) = fixture(Some(2), 0);
        let codecs: Vec<String> = CODECS.iter().map(|s| s.to_string()).collect();
        let r = inspect(&mut io::Cursor::new(&file), 11, offset, offset, &index, &codecs).unwrap();
        assert_eq!(r.methods["Oodle"], 1); assert_eq!(r.compressed, 1); assert_eq!(r.decoded_bytes, 7);
        let mut broken = file.clone(); broken[fdi_off as usize + 4] ^= 1;
        assert!(inspect(&mut io::Cursor::new(broken), 11, offset, offset, &index, &codecs).unwrap_err().to_string().contains("secondary"));
        let mut bad_index = index.clone(); let trailer = bad_index.len() - 4; bad_index[trailer..].copy_from_slice(&1u32.to_le_bytes());
        assert!(inspect(&mut io::Cursor::new(&file), 11, offset, offset, &bad_index, &codecs).is_err());
    }
    #[test]
    fn deleted_sentinels_are_counted_and_reference_errors_fail_closed() {
        let (file, offset, index, _, _) = fixture(None, i32::MIN);
        let r = inspect(&mut io::Cursor::new(&file), 11, offset, offset, &index, &[]).unwrap();
        assert_eq!((r.files, r.deleted), (1, 1)); assert!(r.methods.is_empty());
        let (file, offset, index, _, _) = fixture(None, 4096); // beyond the encoded blob
        assert!(inspect(&mut io::Cursor::new(&file), 11, offset, offset, &index, &[]).is_err());
        let (file, offset, index, _, _) = fixture(None, 0);
        assert!(inspect(&mut io::Cursor::new(&file), 9, offset, offset, &index, &[]).is_err());
        assert!(inspect(&mut io::Cursor::new(&file), 11, offset, offset, &index[..index.len() - 1], &[]).is_err());
    }
    #[test]
    fn encrypted_entries_are_classified_without_hash_or_decrypt_and_block_sizes_consume() {
        let data = b"payload";
        let encoded = encode_entry(Some(2), true, data);
        let mut fdi = Vec::new(); fdi.extend(1u32.to_le_bytes()); string(&mut fdi, "a/");
        fdi.extend(1u32.to_le_bytes()); string(&mut fdi, "b.ubulk"); fdi.extend(0i32.to_le_bytes());
        let mut file = vec![0u8; 16];
        let fdi_off = file.len() as u64; file.extend(&fdi);
        let mut index = Vec::new(); string(&mut index, "/"); index.extend(1u32.to_le_bytes()); index.extend(0u64.to_le_bytes());
        index.extend(0u32.to_le_bytes()); // no path hash index
        index.extend(1u32.to_le_bytes()); index.extend(fdi_off.to_le_bytes()); index.extend((fdi.len() as u64).to_le_bytes()); index.extend(sha(&fdi));
        index.extend((encoded.len() as u32).to_le_bytes()); index.extend(&encoded); index.extend(0u32.to_le_bytes());
        let index_off = file.len() as u64; file.extend(&index);
        let codecs: Vec<String> = CODECS.iter().map(|s| s.to_string()).collect();
        let r = inspect(&mut io::Cursor::new(&file), 11, index_off, index_off, &index, &codecs).unwrap();
        assert_eq!((r.encrypted, r.compressed, r.kinds["ubulk"]), (1, 1, 1));
    }
}
