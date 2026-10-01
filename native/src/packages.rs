//! Lossless recompression of Hades v7 LZ4 packages. Entry bytes, chunk
//! boundaries, XNB textures and the separate atlas manifests are unchanged.
//! Format reference: https://github.com/quaerus/deppth (sggpio/compression).
use lz4::block::{compress, decompress, CompressionMode};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

const CHUNK: usize = 0x0200_0000;
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_CHUNKS: usize = 128;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn prepare(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x20000)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() < 10 || meta.len() > MAX_FILE {
        return Ok(None);
    }
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    if header != [0x20, 0, 0, 7] {
        return Ok(None);
    }
    if std::env::var_os("BGC_VERBOSE").is_some() {
        eprintln!("Lossless package scan: {}", path.display());
    }
    let mut bytes = header.to_vec();
    file.by_ref().take(MAX_FILE - 3).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != meta.len() {
        return Err(invalid("package changed while reading"));
    }
    recompress(&bytes)
}

fn recompress(bytes: &[u8]) -> io::Result<Option<Vec<u8>>> {
    // .pkg is not a universal format. Never rewrite packages for other engines,
    // uncompressed manifests, LZF v5 packages, or unknown version/flag bits.
    if !bytes.starts_with(&[0x20, 0, 0, 7]) {
        return Ok(None);
    }
    let mut output = bytes[..4].to_vec();
    let mut pos = 4usize;
    let mut chunks = 0usize;
    let mut last_progress = Instant::now();
    while pos < bytes.len() {
        super::cancelled()?;
        if chunks >= MAX_CHUNKS {
            return Err(invalid("package exceeds decompression work limit"));
        }
        let capacity = if chunks == 0 { CHUNK - 4 } else { CHUNK };
        let start = pos;
        let flag = bytes[pos];
        pos += 1;
        if flag == 0 {
            // Preserve uncompressed chunks, including their marker and padding.
            pos = pos
                .checked_add(capacity)
                .ok_or_else(|| invalid("package offset overflow"))?;
            if pos > bytes.len() {
                return Err(invalid("truncated raw package chunk"));
            }
            output.extend_from_slice(&bytes[start..pos]);
        } else if flag == 1 {
            let size_bytes = bytes
                .get(pos..pos + 4)
                .ok_or_else(|| invalid("truncated package chunk size"))?;
            let size = u32::from_be_bytes(size_bytes.try_into().unwrap()) as usize;
            pos += 4;
            if size == 0 || size > CHUNK + CHUNK / 255 + 16 {
                return Err(invalid("invalid LZ4 package chunk size"));
            }
            let end = pos
                .checked_add(size)
                .ok_or_else(|| invalid("package offset overflow"))?;
            let encoded = bytes
                .get(pos..end)
                .ok_or_else(|| invalid("truncated LZ4 package chunk"))?;
            let decoded = decompress(encoded, Some(capacity as i32))?;
            if decoded.is_empty() || decoded.len() > capacity {
                return Err(invalid("invalid decoded package chunk size"));
            }
            super::cancelled()?;
            let smaller = compress(&decoded, Some(CompressionMode::HIGHCOMPRESSION(12)), false)?;
            super::cancelled()?;
            if smaller.len() < encoded.len() {
                // Verify every new block before it can enter the backup/apply
                // workflow. This is content-preserving compression, not resizing.
                if decompress(&smaller, Some(capacity as i32))? != decoded {
                    return Err(invalid(
                        "recompressed package failed round-trip verification",
                    ));
                }
                output.push(1);
                output.extend_from_slice(&(smaller.len() as u32).to_be_bytes());
                output.extend_from_slice(&smaller);
            } else {
                output.extend_from_slice(&bytes[start..end]);
            }
            pos = end;
        } else {
            return Err(invalid("unknown package chunk flag"));
        }
        chunks += 1;
        if std::env::var_os("BGC_ASSET_PROGRESS").is_some()
            && last_progress.elapsed() >= Duration::from_secs(2)
        {
            eprintln!("Lossless package scan: verified {chunks} chunk(s), {pos}/{} input bytes; {} potential logical bytes smaller so far.", bytes.len(), pos.saturating_sub(output.len()));
            last_progress = Instant::now();
        }
    }
    if chunks == 0 || output.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(output))
}

/// Bounded warm LZ4 decoder microbenchmark. No input mutation and no caches
/// dropped. Reports best of five runs, excluding I/O and uncompressed chunks.
pub fn audit(path: &Path) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_FILE {
        return Err(invalid("package audit expects a single-link file up to 512 MiB"));
    }
    let mut bytes = Vec::new(); file.by_ref().take(MAX_FILE+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != meta.len() || !bytes.starts_with(&[0x20,0,0,7]) {
        return Err(invalid("unsupported or changing package input"));
    }
    let mut best = f64::INFINITY;
    let mut decoded_bytes = 0usize;
    let mut compressed_chunks = 0usize;
    for _ in 0..5 {
        let mut pos = 4usize;
        let mut chunks = 0usize;
        let mut decoded = 0usize;
        let mut compressed = 0usize;
        let mut elapsed = Duration::ZERO;
        while pos < bytes.len() {
            super::cancelled()?;
            if chunks >= MAX_CHUNKS { return Err(invalid("package chunk count exceeds limit")); }
            let capacity = if chunks == 0 { CHUNK-4 } else { CHUNK };
            let flag = bytes[pos]; pos += 1;
            if flag == 0 {
                pos = pos.checked_add(capacity).ok_or_else(|| invalid("package offset overflow"))?;
                if pos > bytes.len() { return Err(invalid("truncated raw chunk")); }
            } else if flag == 1 {
                let size = bytes.get(pos..pos+4).ok_or_else(|| invalid("truncated chunk length"))?;
                let size = u32::from_be_bytes(size.try_into().unwrap()) as usize; pos += 4;
                if size == 0 || size > CHUNK+CHUNK/255+16 { return Err(invalid("invalid chunk size")); }
                let end = pos.checked_add(size).ok_or_else(|| invalid("package offset overflow"))?;
                let encoded = bytes.get(pos..end).ok_or_else(|| invalid("truncated chunk"))?;
                if decoded+capacity > 512*1024*1024 { return Err(invalid("audit exceeds 512 MiB decoded-work limit per run")); }
                let started = Instant::now();
                let output = decompress(encoded,Some(capacity as i32))?;
                elapsed += started.elapsed();
                if output.is_empty() || output.len() > capacity { return Err(invalid("invalid decoded chunk")); }
                decoded += output.len(); compressed += 1; pos = end;
            } else { return Err(invalid("invalid package chunk flag")); }
            chunks += 1;
        }
        decoded_bytes = decoded; compressed_chunks = compressed;
        best = best.min(elapsed.as_secs_f64());
    }
    if compressed_chunks == 0 { return Err(invalid("no compressed chunks to benchmark")); }
    eprintln!("Warm offline LZ4 decoder benchmark; excludes I/O. Not an in-game loading-time guarantee.");
    println!("PKG_AUDIT|{compressed_chunks}|{decoded_bytes}|{best:.6}|5");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(payloads: &[Vec<u8>]) -> Vec<u8> {
        let mut result = vec![0x20, 0, 0, 7];
        for payload in payloads {
            let encoded = compress(payload, Some(CompressionMode::FAST(1)), false).unwrap();
            result.push(1);
            result.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
            result.extend_from_slice(&encoded);
        }
        result
    }

    fn payloads(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut pos = 4;
        let mut result = Vec::new();
        while pos < bytes.len() {
            assert_eq!(bytes[pos], 1);
            let size = u32::from_be_bytes(bytes[pos + 1..pos + 5].try_into().unwrap()) as usize;
            result.push(decompress(&bytes[pos + 5..pos + 5 + size], Some(CHUNK as i32)).unwrap());
            pos += 5 + size;
        }
        result
    }

    #[test]
    fn recompression_preserves_all_chunks_and_payloads() {
        let mut seed = 12345u32;
        let noise: Vec<u8> = (0..65536)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut repeated = Vec::new();
        for offset in 0..512 {
            repeated.extend_from_slice(&noise[offset..offset + 4096]);
        }
        let source = package(&[repeated, noise]);
        let result = recompress(&source)
            .unwrap()
            .expect("HC should improve fast compression");
        assert!(result.len() < source.len());
        assert_eq!(payloads(&result), payloads(&source));
        assert!(recompress(&result).unwrap().is_none());
    }

    #[test]
    fn unsupported_pkg_formats_are_not_rewritten() {
        for header in [
            [0, 0, 0, 7],
            [0x40, 0, 0, 5],
            [0x20, 0, 0, 8],
            [0x20, 1, 0, 7],
        ] {
            assert!(recompress(&header).unwrap().is_none());
        }
    }

    #[test]
    fn malformed_packages_are_rejected() {
        for suffix in [
            vec![1],
            vec![1, 0xff, 0xff, 0xff, 0xff],
            vec![1, 0, 0, 0, 3, 0, 0, 0],
            vec![0],
            vec![2],
        ] {
            let mut bytes = vec![0x20, 0, 0, 7];
            bytes.extend(suffix);
            assert!(recompress(&bytes).is_err());
        }
    }

    #[test]
    fn audit_reads_packages_without_modifying_them_and_rejects_truncation() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("bgc-pkg-audit-{stamp}.pkg"));
        let original = package(&[vec![42u8;65536]]);
        fs::write(&path,&original).unwrap();
        audit(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(),original);
        fs::write(&path,[0x20,0,0,7,1]).unwrap();
        assert!(audit(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
