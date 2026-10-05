//! Read-only XNB header inventory. Payload readers and decompression are not
//! implemented; recognized headers never imply that contained assets are parsed.
use std::{fs, io::{self, Read}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};

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
    let mut file = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
