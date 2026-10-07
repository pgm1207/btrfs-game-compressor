//! Small, read-only primitives for development format inventories. No extraction,
//! decompression, subprocesses, or writes. Budgets count bytes actually read.
use std::{
    fs::{File, Metadata, OpenOptions},
    io,
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::ffi::OsStrExt,
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

use super::{cancelled, invalid};

#[derive(Debug)]
struct BudgetExceeded(&'static str);
impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for BudgetExceeded {}

pub fn budget_error(message: &'static str) -> io::Error {
    io::Error::other(BudgetExceeded(message))
}
pub fn is_budget_error(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|cause| cause.is::<BudgetExceeded>())
}

/// Open a regular companion beneath an already-open directory, rejecting path
/// traversal and symlinks at every component. Does not establish content
/// ownership, snapshot consistency, or a sandbox against directory relocation
/// and mount manipulation.
pub fn open_beneath(directory: &File, relative: &Path) -> io::Result<File> {
    use std::path::Component;
    if !directory.metadata()?.is_dir() { return Err(invalid("companion root is not a directory")); }
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => parts.push(name),
            Component::CurDir => (),
            _ => return Err(invalid("companion paths must be relative without parent traversal")),
        }
        if parts.len() > 64 { return Err(invalid("companion path depth exceeds limit")); }
    }
    if parts.is_empty() { return Err(invalid("empty companion path")); }
    let mut current = directory.try_clone()?;
    for (index, name) in parts.iter().enumerate() {
        cancelled()?;
        let name = CString::new(name.as_bytes()).map_err(|_| invalid("companion name contains NUL"))?;
        let last = index + 1 == parts.len();
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
            | if last { 0 } else { libc::O_DIRECTORY };
        // One component, anchored to a held descriptor: replacing an earlier
        // pathname with a symlink cannot redirect this component lookup.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 { return Err(io::Error::last_os_error()); }
        let next = unsafe { File::from_raw_fd(fd) };
        let meta = next.metadata()?;
        if last && !meta.is_file() || !last && !meta.is_dir() {
            return Err(invalid("companion path does not name the required regular file/directory"));
        }
        current = next;
    }
    Ok(current)
}

pub struct Source {
    file: File,
    before: Metadata,
    remaining: u64,
}

impl Source {
    pub fn open(path: &Path, max_file: u64, read_budget: u64) -> io::Result<Self> {
        cancelled()?;
        let file = OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
        let before = file.metadata()?;
        if !before.is_file() || before.len() > max_file {
            return Err(invalid("format audit requires a regular file within its size limit"));
        }
        Ok(Self { file, before, remaining: read_budget })
    }

    pub fn len(&self) -> u64 { self.before.len() }

    /// Check a declared range without reading its payload or claiming validity.
    pub fn range(&self, offset: u64, size: u64) -> io::Result<()> {
        if offset.checked_add(size).is_none_or(|end| end > self.len()) {
            return Err(invalid("format audit extent exceeds source bounds"));
        }
        Ok(())
    }

    pub fn read_at(&mut self, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        cancelled()?;
        self.range(offset, size as u64)?;
        self.remaining = self.remaining.checked_sub(size as u64)
            .ok_or_else(|| budget_error("format audit read budget exceeded"))?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| invalid("format audit allocation limit exceeded"))?;
        bytes.resize(size, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        cancelled()?;
        Ok(bytes)
    }

    pub fn u8(&mut self, offset: &mut u64) -> io::Result<u8> {
        let value = self.read_at(*offset, 1)?[0];
        *offset = offset.checked_add(1).ok_or_else(|| invalid("audit offset overflow"))?;
        Ok(value)
    }

    pub fn u32(&mut self, offset: &mut u64) -> io::Result<u32> {
        let bytes = self.read_at(*offset, 4)?;
        *offset = offset.checked_add(4).ok_or_else(|| invalid("audit offset overflow"))?;
        Ok(u32::from_le_bytes(bytes.as_slice().try_into().unwrap()))
    }

    /// Check the opened inode, not a later lookup of its pathname. This is change
    /// detection, not a filesystem snapshot or a guarantee against hostile writes.
    pub fn unchanged(&self) -> io::Result<()> {
        cancelled()?;
        let after = self.file.metadata()?;
        if self.before.dev() != after.dev() || self.before.ino() != after.ino()
            || self.before.len() != after.len() || self.before.mtime() != after.mtime()
            || self.before.mtime_nsec() != after.mtime_nsec() || self.before.ctime() != after.ctime()
            || self.before.ctime_nsec() != after.ctime_nsec() {
            return Err(invalid("format audit source changed during inspection"));
        }
        Ok(())
    }
}

/// Reversible byte escaping for untrusted fields in pipe-delimited records.
/// ASCII control characters, percent, pipes, backslashes and non-ASCII bytes
/// cannot create records, terminal escapes or ambiguous escape sequences.
pub fn field(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for &b in bytes {
        if (0x20..=0x7e).contains(&b) && !matches!(b, b'%' | b'|' | b'\\') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}

pub fn le16(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let end = offset.checked_add(2).ok_or_else(|| invalid("audit field offset overflow"))?;
    let value = bytes.get(offset..end).ok_or_else(|| invalid("truncated audit field"))?;
    Ok(u16::from_le_bytes(value.try_into().unwrap()))
}

pub fn le32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset.checked_add(4).ok_or_else(|| invalid("audit field offset overflow"))?;
    let value = bytes.get(offset..end).ok_or_else(|| invalid("truncated audit field"))?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

/// Summarize already-bounds-checked declared ranges in one address space. A union
/// is not ownership, content equality, removable space or physical disk savings.
pub struct RangeSummary {
    pub bytes: u64, pub union: u64, pub aliases: u64, pub overlap_groups: u64,
}
pub fn summarize_ranges(ranges: &mut [(u64, u64)]) -> io::Result<RangeSummary> {
    cancelled()?;
    ranges.sort_unstable();
    let mut result = RangeSummary { bytes: 0, union: 0, aliases: 0, overlap_groups: 0 };
    let mut active = None::<(u64, u64)>;
    let mut previous = None;
    let mut overlapping = false;
    for (index, &(start, end)) in ranges.iter().enumerate() {
        if index % 1024 == 0 { cancelled()?; }
        let size = end.checked_sub(start).ok_or_else(|| invalid("inverted audit extent"))?;
        result.bytes = result.bytes.checked_add(size).ok_or_else(|| invalid("audit declared-range total overflow"))?;
        if size == 0 { continue; }
        if previous == Some((start, end)) { result.aliases += 1; }
        match active {
            Some((first, last)) if last > start => {
                if previous != Some((start, end)) { overlapping = true; }
                active = Some((first, last.max(end)));
            }
            _ => {
                if let Some((first, last)) = active {
                    result.union = result.union.checked_add(last - first)
                        .ok_or_else(|| invalid("audit range-union total overflow"))?;
                    result.overlap_groups += u64::from(overlapping);
                }
                active = Some((start, end)); overlapping = false;
            }
        }
        previous = Some((start, end));
    }
    if let Some((first, last)) = active {
        result.union = result.union.checked_add(last - first)
            .ok_or_else(|| invalid("audit range-union total overflow"))?;
        result.overlap_groups += u64::from(overlapping);
    }
    Ok(result)
}

/// Container-neutral storage shape, not an engine's numeric format identifier.
/// Channel ordering, alpha and color-space semantics must remain adapter-specific.
#[derive(Clone, Copy)]
pub struct Layout { pub block_width: u32, pub block_height: u32, pub block_bytes: u64 }

impl Layout {
    pub const fn pixels(bytes: u64) -> Self {
        Self { block_width: 1, block_height: 1, block_bytes: bytes }
    }
    pub const fn bc(bytes: u64) -> Self {
        Self { block_width: 4, block_height: 4, block_bytes: bytes }
    }
    pub fn bytes(self, width: u32, height: u32) -> io::Result<u64> {
        if width == 0 || height == 0 || self.block_width == 0 || self.block_height == 0
            || self.block_bytes == 0 {
            return Err(invalid("invalid audit texture storage shape"));
        }
        (width.div_ceil(self.block_width) as u64)
            .checked_mul(height.div_ceil(self.block_height) as u64)
            .and_then(|blocks| blocks.checked_mul(self.block_bytes))
            .ok_or_else(|| invalid("audit texture storage overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::fs::symlink, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let name = format!("bgc-audit-io-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
            let root = std::env::temp_dir().join(name);
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn source_enforces_read_budget_bounds_and_change_detection() {
        let fixture = Fixture::new();
        let path = fixture.0.join("bytes.bin");
        fs::write(&path, b"abcdefgh").unwrap();
        let mut source = Source::open(&path, 8, 4).unwrap();
        assert_eq!(source.read_at(2, 3).unwrap(), b"cde");
        assert!(is_budget_error(&source.read_at(0, 2).unwrap_err()));
        assert!(source.range(7, 2).is_err());
        assert!(source.range(u64::MAX, 2).is_err());
        assert!(Source::open(&path, 7, 8).is_err());
        let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writer.write_all(b"i").unwrap();
        assert!(source.unchanged().is_err());
    }

    #[test]
    fn companion_open_rejects_symlinks_and_traversal() {
        let fixture = Fixture::new();
        let nested = fixture.0.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("asset.bin"), b"data").unwrap();
        symlink("nested", fixture.0.join("linked-dir")).unwrap();
        symlink("nested/asset.bin", fixture.0.join("linked-file")).unwrap();
        let directory = File::open(&fixture.0).unwrap();
        assert_eq!(open_beneath(&directory, Path::new("nested/asset.bin")).unwrap().metadata().unwrap().len(), 4);
        for name in ["linked-dir/asset.bin", "linked-file", "../asset.bin", "/etc/passwd", "nested"] {
            assert!(open_beneath(&directory, Path::new(name)).is_err(), "{name}");
        }
    }

    #[test]
    fn record_fields_escape_untrusted_bytes() {
        assert_eq!(field(b"a|b%\\\n\x00\xff"), "a%7Cb%25%5C%0A%00%FF");
    }
}
