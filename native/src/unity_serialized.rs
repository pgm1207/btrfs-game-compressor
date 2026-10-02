//! Read-only Unity SerializedFile v17–22 metadata inventory. Object payloads
//! are opaque: class IDs do not establish texture codec/layout or safe resizing.
//! Format references: UnityDataTools playerbuild-format.md and AssetStudio's
//! SerializedFile.cs. Independent bounded reader; no runtime SDK dependency.
use std::{collections::{BTreeMap, BTreeSet}, fs, io::{self, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};

const MAX_METADATA: u64 = 32 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
fn bad(s: &str) -> io::Error { super::invalid(s) }

struct Cursor<'a> { bytes: &'a [u8], pos: usize, big: bool }
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("Unity metadata offset overflow"))?;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| bad("truncated Unity metadata"))?;
        self.pos = end; Ok(bytes)
    }
    fn u8(&mut self) -> io::Result<u8> { Ok(self.take(1)?[0]) }
    fn boolean(&mut self) -> io::Result<bool> {
        match self.u8()? { 0 => Ok(false), 1 => Ok(true), _ => Err(bad("invalid Unity boolean")) }
    }
    fn i16(&mut self) -> io::Result<i16> {
        let b = self.take(2)?.try_into().unwrap();
        Ok(if self.big { i16::from_be_bytes(b) } else { i16::from_le_bytes(b) })
    }
    fn u32(&mut self) -> io::Result<u32> {
        let b = self.take(4)?.try_into().unwrap();
        Ok(if self.big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }
    fn i32(&mut self) -> io::Result<i32> { Ok(self.u32()? as i32) }
    fn u64(&mut self) -> io::Result<u64> {
        let b = self.take(8)?.try_into().unwrap();
        Ok(if self.big { u64::from_be_bytes(b) } else { u64::from_le_bytes(b) })
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = self.u32()? as usize;
        if n > MAX_ITEMS { return Err(bad("Unity metadata count exceeds limit")); }
        Ok(n)
    }
    fn string(&mut self, limit: usize) -> io::Result<String> {
        let tail = &self.bytes[self.pos..];
        let n = tail.iter().take(limit + 1).position(|b| *b == 0)
            .ok_or_else(|| bad("Unity string missing terminator or exceeds limit"))?;
        let text = std::str::from_utf8(self.take(n)?).map_err(|_| bad("invalid Unity UTF-8 metadata"))?.to_owned();
        self.take(1)?;
        if text.chars().any(|c| c.is_control() || c == '|') { return Err(bad("unsafe Unity metadata text")); }
        Ok(text)
    }
    fn align(&mut self) -> io::Result<()> {
        let n = (4 - self.pos % 4) % 4;
        self.take(n)?; Ok(())
    }
}

#[derive(Debug)]
struct Header { version: u32, metadata: u64, data: u64, size: u64, start: u64, big: bool }
/// SerializedFiles have no strong magic. Only route plausible modern headers;
/// full parsing still verifies length, bounds, tables and referenced types.
pub fn plausible(bytes: &[u8]) -> bool {
    bytes.len() >= 20 && (17..=22).contains(&u32::from_be_bytes(bytes[8..12].try_into().unwrap()))
        && bytes[16] <= 1 && bytes[17..20] == [0; 3]
}
fn header(bytes: &[u8], length: u64) -> io::Result<Header> {
    if !plausible(bytes) { return Err(bad("unsupported Unity SerializedFile header (requires v17–22)")); }
    let mut c = Cursor { bytes, pos: 0, big: true };
    let mut metadata = c.u32()? as u64;
    let mut size = c.u32()? as u64;
    let version = c.u32()?;
    let mut data = c.u32()? as u64;
    let big = c.boolean()?;
    c.take(3)?;
    if version == 22 {
        metadata = c.u32()? as u64; size = c.u64()?; data = c.u64()?;
        if c.u64()? != 0 { return Err(bad("unknown Unity v22 header extension")); }
    }
    let start = c.pos as u64;
    if size != length || metadata == 0 || metadata > MAX_METADATA
        || start.checked_add(metadata).is_none_or(|end| end > data)
        || data > size || data % 4 != 0 {
        return Err(bad("invalid Unity file/metadata/data bounds"));
    }
    Ok(Header { version, metadata, data, size, start, big })
}

fn serialized_type(c: &mut Cursor<'_>, version: u32, trees: bool, reference: bool) -> io::Result<i32> {
    let class = c.i32()?;
    c.boolean()?; // stripped type marker
    let script = c.i16()?;
    if class == 114 || reference && script >= 0 { c.take(16)?; }
    c.take(16)?; // original type hash
    if trees {
        let nodes = c.count()?;
        let strings = c.count()?;
        let stride = if version >= 19 { 32 } else { 24 };
        c.take(nodes.checked_mul(stride).ok_or_else(|| bad("Unity type-tree size overflow"))?)?;
        c.take(strings)?;
        if version >= 21 {
            if reference {
                for _ in 0..3 { c.string(4096)?; }
            } else {
                let count = c.count()?;
                c.take(count.checked_mul(4).ok_or_else(|| bad("Unity dependencies overflow"))?)?;
            }
        }
    }
    Ok(class)
}

#[derive(Debug)]
struct Inventory {
    engine: String, platform: i32, trees: bool, types: usize, objects: usize,
    classes: BTreeMap<i32, (u64, u64)>, external: usize,
}
fn inventory(bytes: &[u8], h: &Header) -> io::Result<Inventory> {
    let mut c = Cursor { bytes, pos: 0, big: h.big };
    let engine = c.string(128)?;
    let platform = c.i32()?;
    let trees = c.boolean()?;
    let ntypes = c.count()?;
    let mut types = Vec::with_capacity(ntypes.min(4096));
    for _ in 0..ntypes { types.push(serialized_type(&mut c, h.version, trees, false)?); super::cancelled()?; }
    let objects = c.count()?;
    let mut classes = BTreeMap::<i32, (u64, u64)>::new();
    let mut ranges = Vec::new(); let mut ids = BTreeSet::new();
    for _ in 0..objects {
        super::cancelled()?; c.align()?;
        if !ids.insert(c.u64()?) { return Err(bad("duplicate Unity object path ID")); }
        let offset = if h.version == 22 { c.u64()? } else { c.u32()? as u64 };
        let size = c.u32()? as u64;
        let type_id = c.count()?;
        let class = *types.get(type_id).ok_or_else(|| bad("Unity object has invalid type index"))?;
        let start = h.data.checked_add(offset).ok_or_else(|| bad("Unity object offset overflow"))?;
        let end = start.checked_add(size).ok_or_else(|| bad("Unity object length overflow"))?;
        if end > h.size { return Err(bad("Unity object exceeds file bounds")); }
        if size > 0 { ranges.push((start, end)); }
        let totals = classes.entry(class).or_default(); totals.0 += 1; totals.1 += size;
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|r| r[0].1 > r[1].0) { return Err(bad("overlapping Unity objects")); }
    let scripts = c.count()?;
    for _ in 0..scripts { c.i32()?; c.align()?; c.u64()?; }
    let external = c.count()?;
    for _ in 0..external { c.string(4096)?; c.take(16)?; c.i32()?; c.string(4096)?; }
    if h.version >= 20 {
        let references = c.count()?;
        for _ in 0..references { serialized_type(&mut c, h.version, trees, true)?; super::cancelled()?; }
    }
    c.string(4096)?; // user information
    if c.bytes[c.pos..].iter().any(|b| *b != 0) { return Err(bad("unknown Unity metadata trailer")); }
    Ok(Inventory { engine, platform, trees, types: ntypes, objects, classes, external })
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() { return Err(bad("Unity audit requires a regular file")); }
    let mut prefix = [0u8; 48];
    let n = file.read(&mut prefix)?;
    let h = header(&prefix[..n], meta.len())?;
    let mut bytes = vec![0; h.metadata as usize];
    file.seek(SeekFrom::Start(h.start))?; file.read_exact(&mut bytes)?;
    let v = inventory(&bytes, &h)?;
    let after = file.metadata()?;
    if meta.len() != after.len() || meta.mtime() != after.mtime() || meta.mtime_nsec() != after.mtime_nsec()
        || meta.ctime() != after.ctime() || meta.ctime_nsec() != after.ctime_nsec() {
        return Err(bad("Unity source changed during audit"));
    }
    println!("UNITY_SERIALIZED|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}", h.version, h.size, h.metadata, h.data,
        if h.big { "big" } else { "little" }, v.engine, v.platform, u8::from(v.trees), v.types, v.objects, v.external);
    for (class, (count, bytes)) in v.classes {
        let kind = match class { 28 => "Texture2D", 83 => "AudioClip", 213 => "Sprite", 687078895 => "SpriteAtlas", _ => "opaque" };
        println!("UNITY_CLASS|{class}|{kind}|{count}|{bytes}");
    }
    eprintln!("Read-only Unity metadata: class counts are not codec/dimension/stream-size inventory. Object payloads and external files are untouched. Type trees absent requires validated version-specific schemas before rewriting; texture/audio apply is not implemented.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put32(b: &mut Vec<u8>, v: u32, big: bool) { b.extend(if big { v.to_be_bytes() } else { v.to_le_bytes() }); }
    fn put64(b: &mut Vec<u8>, v: u64, big: bool) { b.extend(if big { v.to_be_bytes() } else { v.to_le_bytes() }); }
    fn fixture(version: u32, big: bool, trees: bool) -> (Header, Vec<u8>) {
        let mut b = b"2022.3.0f1\0".to_vec(); put32(&mut b, 19, big); b.push(u8::from(trees)); put32(&mut b, 2, big);
        for class in [28, 83] {
            put32(&mut b, class, big); b.push(0); b.extend([255; 2]); b.extend([0; 16]);
            if trees {
                put32(&mut b, 1, big); put32(&mut b, 4, big);
                b.extend(vec![0; if version >= 19 { 32 } else { 24 }]); b.extend(b"x\0\0\0");
                if version >= 21 { put32(&mut b, 0, big); }
            }
        }
        put32(&mut b, 2, big);
        for id in 0..2 {
            while b.len() % 4 != 0 { b.push(0); }
            put64(&mut b, id + 1, big);
            if version == 22 { put64(&mut b, id * 16, big); } else { put32(&mut b, id as u32 * 16, big); }
            put32(&mut b, 16, big); put32(&mut b, id as u32, big);
        }
        put32(&mut b, 0, big); put32(&mut b, 1, big);
        b.push(0); b.extend([0; 16]); put32(&mut b, 0, big); b.extend(b"other.assets\0");
        if version >= 20 { put32(&mut b, 0, big); }
        b.push(0);
        let start = if version == 22 { 48 } else { 20 };
        let data = (start + b.len() as u64).div_ceil(16) * 16;
        (Header { version, metadata: b.len() as u64, data, size: data + 32, start, big }, b)
    }
    #[test]
    fn versioned_tables_work_with_both_endians_and_with_or_without_trees() {
        for version in 17..=22 { for big in [false, true] { for trees in [false, true] {
            let (h, bytes) = fixture(version, big, trees);
            let v = inventory(&bytes, &h).unwrap();
            assert_eq!((v.objects, v.external, v.types, v.trees), (2, 1, 2, trees));
            assert_eq!(v.classes[&28], (1, 16)); assert_eq!(v.classes[&83], (1, 16));
        } } }
    }
    #[test]
    fn rejects_every_metadata_truncation_and_out_of_bounds_payloads() {
        let (h, bytes) = fixture(22, false, true);
        for n in 0..bytes.len() { assert!(inventory(&bytes[..n], &h).is_err(), "{n}"); }
        let h = Header { size: h.data + 31, ..h };
        assert!(inventory(&bytes, &h).is_err());
    }
    #[test]
    fn header_bounds_and_future_versions_fail_closed() {
        let (h, _) = fixture(22, false, false);
        let mut bytes = vec![0; 48]; bytes[8..12].copy_from_slice(&22u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&(h.metadata as u32).to_be_bytes());
        bytes[24..32].copy_from_slice(&h.size.to_be_bytes()); bytes[32..40].copy_from_slice(&h.data.to_be_bytes());
        assert!(header(&bytes, h.size).is_ok());
        assert!(header(&bytes, h.size + 1).is_err());
        bytes[8..12].copy_from_slice(&23u32.to_be_bytes()); assert!(!plausible(&bytes));
        bytes[8..12].copy_from_slice(&22u32.to_be_bytes()); bytes[16] = 2; assert!(!plausible(&bytes));
    }
    #[test]
    fn invalid_types_duplicate_ids_overlaps_and_hostile_strings_are_rejected() {
        let (h, bytes) = fixture(22, false, false);
        let mut c = Cursor { bytes: &bytes, pos: 0, big: false };
        c.string(128).unwrap(); c.i32().unwrap(); c.boolean().unwrap();
        for _ in 0..c.count().unwrap() { serialized_type(&mut c, 22, false, false).unwrap(); }
        c.count().unwrap(); c.align().unwrap();
        let first = c.pos; let second = first + 24;
        let mut broken = bytes.clone(); broken[first + 20..first + 24].copy_from_slice(&99u32.to_le_bytes());
        assert!(inventory(&broken, &h).is_err());
        let mut broken = bytes.clone(); broken[second..second + 8].copy_from_slice(&1u64.to_le_bytes());
        assert!(inventory(&broken, &h).is_err());
        let mut broken = bytes.clone(); broken[second + 8..second + 16].copy_from_slice(&8u64.to_le_bytes());
        assert!(inventory(&broken, &h).is_err());
        let mut broken = bytes.clone(); broken[0] = b'|'; assert!(inventory(&broken, &h).is_err());
    }
}
