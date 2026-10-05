//! Read-only Unreal IoStore `.utoc` header inventory. Reports container counts,
//! flags, compression method metadata and sibling presence; it does not parse
//! chunk arrays, decompress `.ucas`, or write anything. Layout reference:
//! retoc/unreal_asset/UEcastoc documentation (no runtime dependency).
use std::{fs, io::{self, Read}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};
fn bad(s: &str) -> io::Error { super::invalid(s) }
pub const MAGIC: &[u8; 16] = b"-==--==--==--==-";

fn u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn u64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }
fn is_power_of_two(v: u32) -> bool { v != 0 && v & (v - 1) == 0 }

#[derive(Debug, Default)]
pub struct Header {
    pub version: u8, pub entries: u32, pub blocks: u32, pub block_size: u32,
    pub method_count: u32, pub method_len: u32, pub dir_index_size: u32,
    pub partitions: u32, pub container_id: u64, pub flags: u8,
    pub perfect_seeds: u32, pub without_perfect: u32, pub partition_size: u64,
}
impl Header {
    pub fn encrypted(&self) -> bool { self.flags & 2 != 0 }
    pub fn signed(&self) -> bool { self.flags & 4 != 0 }
    pub fn compressed(&self) -> bool { self.flags & 1 != 0 }
    pub fn indexed(&self) -> bool { self.flags & 8 != 0 }
    /// A true lower bound on the `.utoc` size from header-known sections. The
    /// chunk offset/length array and any version-specific tables are not parsed,
    /// so the residual is expected and is reported rather than asserted.
    pub fn lower_bound(&self) -> Option<u64> {
        let mut total = 144u64;
        // Chunk IDs (12) plus offset and length (two 5-byte fields).
        total = total.checked_add(u64::from(self.entries) * 22)?;
        total = total.checked_add(u64::from(self.blocks) * 12)?;
        if self.version >= 4 { total = total.checked_add(u64::from(self.perfect_seeds) * 4)?; }
        if self.version >= 5 { total = total.checked_add(u64::from(self.without_perfect) * 4)?; }
        total = total.checked_add(u64::from(self.method_count) * u64::from(self.method_len))?;
        let meta_size = if self.version >= 8 { 24 } else { 33 };
        total = total.checked_add(u64::from(self.entries) * meta_size)?;
        // Signed TOCs embed a length word and one SHA1 per block, plus
        // two variable-length signatures which this header audit never reads.
        if self.signed() { total = total.checked_add(4 + u64::from(self.blocks) * 20)?; }
        total.checked_add(u64::from(self.dir_index_size))
    }
}
pub fn parse(bytes: &[u8]) -> io::Result<Header> {
    if bytes.len() < 144 || &bytes[..16] != MAGIC { return Err(bad("not an Unreal IoStore .utoc header")); }
    let version = bytes[16];
    if version == 0 || version > 8 { return Err(bad("unsupported Unreal IoStore TOC version")); }
    let h = Header {
        version, entries: u32(bytes, 24), blocks: u32(bytes, 28),
        method_count: u32(bytes, 36), method_len: u32(bytes, 40), block_size: u32(bytes, 44),
        dir_index_size: u32(bytes, 48), partitions: u32(bytes, 52), container_id: u64(bytes, 56),
        flags: bytes[80], perfect_seeds: u32(bytes, 84), partition_size: u64(bytes, 88),
        without_perfect: u32(bytes, 96),
    };
    if u32(bytes, 20) != 144 { return Err(bad("unsupported Unreal IoStore header size")); }
    if h.flags & !15 != 0 { return Err(bad("unsupported Unreal IoStore container flags")); }
    if h.entries > 20_000_000 || h.blocks > 100_000_000 { return Err(bad("Unreal IoStore counts exceed limits")); }
    if u32(bytes, 32) != 12 { return Err(bad("unsupported Unreal IoStore compressed-block-entry size")); }
    if h.method_count > 8 || h.method_len > 32 || (h.method_count > 0 && h.method_len == 0) {
        return Err(bad("unsupported Unreal IoStore compression-method table"));
    }
    if !is_power_of_two(h.block_size) || !(0x1000..=0x100_0000).contains(&h.block_size) {
        return Err(bad("unsupported Unreal IoStore compression block size"));
    }
    Ok(h)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() { return Err(bad("IoStore audit requires a regular file")); }
    let mut bytes = [0u8; 144];
    file.read_exact(&mut bytes)?;
    let h = parse(&bytes)?;
    if u64::from(h.dir_index_size) > meta.len() { return Err(bad("Unreal IoStore directory index exceeds file bounds")); }
    let after = file.metadata()?;
    if meta.len() != after.len() || meta.mtime() != after.mtime() || meta.mtime_nsec() != after.mtime_nsec()
        || meta.ctime() != after.ctime() || meta.ctime_nsec() != after.ctime_nsec() {
        return Err(bad("Unreal IoStore source changed during audit"));
    }
    let sibling = |ext: &str| -> io::Result<Option<u64>> {
        match fs::symlink_metadata(path.with_extension(ext)) {
            Ok(m) if m.is_file() => Ok(Some(m.len())), Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None), Err(e) => Err(e),
        }
    };
    let ucas = sibling("ucas")?; let sig = sibling("sig")?.is_some(); let stub = sibling("pak")?.is_some();
    let lower = h.lower_bound();
    if lower.is_none_or(|v| v > meta.len()) { return Err(bad("Unreal IoStore header implies more bytes than the file holds")); }
    let lower = lower.unwrap();
    println!("UNREAL_IOSTORE|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}", h.version, meta.len(), h.entries, h.blocks,
        h.block_size, h.method_count, h.method_len, h.dir_index_size, h.partitions, h.flags, h.container_id, h.partition_size);
    println!("UNREAL_IOSTORE_LAYOUT|{}|{}|{}", meta.len(), lower, meta.len() - lower);
    println!("UNREAL_IOSTORE_UCAS|{}|{}", u8::from(ucas.is_some()), ucas.unwrap_or(0));
    println!("UNREAL_IOSTORE_SECURITY|{}|{}|{}|{}|{}|{}", u8::from(h.encrypted()), u8::from(h.signed()),
        u8::from(h.compressed()), u8::from(h.indexed()), u8::from(sig), u8::from(stub));
    println!("UNREAL_IOSTORE_HASH|{}|{}", h.perfect_seeds, h.without_perfect);
    eprintln!("Read-only Unreal IoStore header: chunk IDs, offsets, compression blocks and the directory index are not parsed. `.ucas` payloads are never opened; encryption/signing flags are reported without decrypting or verifying signatures. No IoStore writer exists; IoStore-backed UE5 cooked textures require chunk/package parsing, not merely Pak entry inspection.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(extra: usize) -> Vec<u8> {
        let h = Header { version: 8, entries: 2, blocks: 2, block_size: 0x10000, method_count: 1, method_len: 32,
            dir_index_size: 4, partitions: 1, container_id: 0x1122334455667788, flags: 8,
            perfect_seeds: 2, without_perfect: 0, partition_size: 0 };
        let mut b = vec![0u8; 144];
        b[..16].copy_from_slice(MAGIC); b[16] = h.version;
        b[20..24].copy_from_slice(&144u32.to_le_bytes());
        b[24..28].copy_from_slice(&h.entries.to_le_bytes()); b[28..32].copy_from_slice(&h.blocks.to_le_bytes());
        b[32..36].copy_from_slice(&12u32.to_le_bytes()); b[36..40].copy_from_slice(&h.method_count.to_le_bytes());
        b[40..44].copy_from_slice(&h.method_len.to_le_bytes()); b[44..48].copy_from_slice(&h.block_size.to_le_bytes());
        b[48..52].copy_from_slice(&h.dir_index_size.to_le_bytes()); b[52..56].copy_from_slice(&h.partitions.to_le_bytes());
        b[56..64].copy_from_slice(&h.container_id.to_le_bytes()); b[80] = h.flags;
        b[84..88].copy_from_slice(&h.perfect_seeds.to_le_bytes()); b[88..96].copy_from_slice(&h.partition_size.to_le_bytes());
        b[96..100].copy_from_slice(&h.without_perfect.to_le_bytes());
        b.resize(h.lower_bound().unwrap() as usize + extra, 0); b
    }
    #[test]
    fn header_fields_parse_and_lower_bound_is_consistent() {
        let bytes = fixture(5);
        let h = parse(&bytes).unwrap();
        assert_eq!((h.version, h.entries, h.blocks, h.block_size), (8, 2, 2, 0x10000));
        assert_eq!((h.method_count, h.method_len, h.dir_index_size), (1, 32, 4));
        assert_eq!(h.container_id, 0x1122334455667788); assert!(h.indexed()); assert!(!h.encrypted());
        assert!(h.lower_bound().unwrap() <= bytes.len() as u64);
        assert_eq!(h.lower_bound(), Some(144 + 2 * 22 + 2 * 12 + 2 * 4 + 32 + 2 * 24 + 4));
    }
    #[test]
    fn hostile_headers_fail_closed() {
        let good = fixture(24);
        let mut b = good.clone(); b[16] = 0; assert!(parse(&b).is_err());
        let mut b = good.clone(); b[16] = 9; assert!(parse(&b).is_err());
        let mut b = good.clone(); b[20..24].copy_from_slice(&160u32.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[32..36].copy_from_slice(&16u32.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[44..48].copy_from_slice(&0x1001u32.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[40..44].copy_from_slice(&40u32.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[36..40].copy_from_slice(&9u32.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[24..28].copy_from_slice(&u32::MAX.to_le_bytes()); assert!(parse(&b).is_err());
        let mut b = good.clone(); b[28..32].copy_from_slice(&u32::MAX.to_le_bytes()); assert!(parse(&b).is_err());
        assert!(parse(b"short").is_err());
        for n in 0..144 { assert!(parse(&good[..n]).is_err()); }
        let mut b = good.clone(); b[80] = 0x80; assert!(parse(&b).is_err());
    }
    #[test]
    fn metadata_versions_and_embedded_signatures_adjust_bounds() {
        let mut h = parse(&fixture(0)).unwrap();
        let v8 = h.lower_bound().unwrap();
        h.version = 6; assert_eq!(h.lower_bound(), Some(v8 + 2 * 9));
        h.flags |= 4; assert_eq!(h.lower_bound(), Some(v8 + 2 * 9 + 4 + 2 * 20));
        h.version = 3; assert_eq!(h.lower_bound(), Some(v8 + 2 * 9 + 4 + 2 * 20 - 2 * 4));
    }
}
