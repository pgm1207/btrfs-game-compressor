//! Linux Btrfs backend. No subprocesses and no third-party crates.
//! UAPI layouts: linux/{btrfs,fiemap,fs}.h. Only 64-bit Linux is supported.
mod assets;
mod packages;
mod fmod;
mod variants;
mod engines;
mod unityfs;
mod containers;
mod gst2;
mod texture_policy;
mod audio_policy;
mod godot3;
mod gdst;
mod md5;
#[cfg(not(all(
    target_os = "linux",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
compile_error!("bgc-native supports x86_64 and aarch64 Linux");

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
    env,
    ffi::{c_int, c_ulong, c_void, CString},
    fs::{self, File, Metadata, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, BufReader, BufWriter, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn syscall(number: c_ulong, ...) -> i64;
    fn signal(sig: c_int, handler: usize) -> usize;
}
static SIGNAL: AtomicUsize = AtomicUsize::new(0);
extern "C" fn stop(sig: c_int) {
    SIGNAL.store(sig as usize, Ordering::Relaxed);
}
fn cancelled() -> io::Result<()> {
    if SIGNAL.load(Ordering::Relaxed) != 0 {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation interrupted",
        ))
    } else {
        Ok(())
    }
}
fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
// Optional `--debug` plus game-relative paths for the prune subcommands.
fn prune_args(args: &[std::ffi::OsString]) -> io::Result<(bool, Vec<PathBuf>)> {
    let (mut debug, mut rels) = (false, Vec::new());
    for arg in args {
        let text = arg.to_str().ok_or_else(|| invalid("prune path is not valid UTF-8"))?;
        if text == "--debug" {
            debug = true;
        } else {
            rels.push(PathBuf::from(text));
        }
    }
    if !debug && rels.is_empty() {
        return Err(invalid("prune requires --debug and/or a game-relative path"));
    }
    Ok((debug, rels))
}
fn request(dir: u64, kind: u64, nr: u64, size: u64) -> c_ulong {
    ((dir << 30) | (size << 16) | (kind << 8) | nr) as c_ulong
}
fn call<T>(file: &File, req: c_ulong, data: &mut T) -> io::Result<()> {
    // The caller supplies a repr(C) layout or an aligned byte buffer of the UAPI size.
    if unsafe { ioctl(file.as_raw_fd(), req, data as *mut T as *mut c_void) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn ne64(b: &[u8], n: usize) -> u64 {
    u64::from_ne_bytes(b[n..n + 8].try_into().unwrap())
}
fn ne32(b: &[u8], n: usize) -> u32 {
    u32::from_ne_bytes(b[n..n + 4].try_into().unwrap())
}
fn le64(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes(b[n..n + 8].try_into().unwrap())
}
fn bytes<T>(v: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, std::mem::size_of::<T>()) }
}

struct Tree {
    root: File,
    display: PathBuf,
    dev: u64,
    sector: u64,
}
impl Tree {
    fn new(path: &Path) -> io::Result<Self> {
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(0x20000 | 0x10000)
            .open(path)?; // NOFOLLOW | DIRECTORY
        let mut info = [0u64; 128];
        call(&root, request(2, 0x94, 31, 1024), &mut info)?;
        let sector = ne32(bytes(&info), 36) as u64;
        if !sector.is_power_of_two() || !(4096..=65536).contains(&sector) {
            return Err(invalid("unsupported Btrfs sector size"));
        }
        let dev = root.metadata()?.dev();
        Ok(Self {
            root,
            display: path.to_owned(),
            dev,
            sector,
        })
    }
    fn open(&self, path: &Path, write: bool) -> io::Result<File> {
        let name =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid("NUL in filename"))?;
        // openat2: pin traversal beneath the selected root; never follow symlinks,
        // magic links or nested mounts. NONBLOCK prevents blocking on a replaced FIFO.
        let how = [
            if write { 2u64 } else { 0 } | 0x80000 | 0x800,
            0,
            0x08 | 0x04 | 0x01,
        ];
        let fd = unsafe {
            syscall(
                437,
                self.root.as_raw_fd(),
                name.as_ptr(),
                how.as_ptr(),
                24usize,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd as c_int) })
    }
    fn walk(&self, mut visit: impl FnMut(&Path, File) -> io::Result<()>) -> io::Result<()> {
        let mut dirs = vec![PathBuf::from(".")];
        let mut seen = HashSet::new();
        while let Some(dir) = dirs.pop() {
            cancelled()?;
            let fd = self.open(&dir, false)?;
            for ent in fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd()))? {
                cancelled()?;
                let ent = ent?;
                let typ = ent.file_type()?;
                if !typ.is_dir() && !typ.is_file() {
                    continue;
                }
                let path = dir.join(ent.file_name());
                if ent.file_name() == ".bgc-assets-backup" {
                    continue;
                }
                let file = match self.open(&path, false) {
                    Ok(f) => f,
                    Err(e) if matches!(e.raw_os_error(), Some(18 | 40)) => continue, // mount / symlink
                    Err(e) => return Err(e),
                };
                let meta = file.metadata()?;
                if meta.dev() != self.dev {
                    continue;
                }
                if meta.is_dir() {
                    dirs.push(path);
                } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
                    visit(&path, file)?;
                }
            }
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Default)]
struct Defrag {
    start: u64,
    len: u64,
    flags: u64,
    threshold: u32,
    compression: [u8; 4],
    unused: [u32; 4],
}
pub(crate) fn compress_file(file: &File, path: &Path, level: u8) -> io::Result<()> {
    compress_file_progress(file, path, level, |_| {})
}
fn compress_file_progress(
    file: &File,
    path: &Path,
    level: u8,
    mut progress: impl FnMut(u64),
) -> io::Result<()> {
    if !(1..=15).contains(&level) {
        return Err(invalid("ZSTD level must be 1..15"));
    }
    let size = file.metadata()?.len();
    let mut start = 0;
    while start < size {
        cancelled()?;
        let len = (size - start).min(32 * 1024 * 1024);
        let mut args = Defrag {
            start,
            len,
            flags: 1 | 2 | 4,
            threshold: 1,
            compression: [3, level, 0, 0],
            ..Default::default()
        };
        call(file, request(1, 0x94, 16, 48), &mut args).map_err(|e| {
            // Keep ENOTTY/EOPNOTSUPP intact so best-effort callers can tell
            // "this filesystem/kernel has no such ioctl" apart from a real
            // compression failure, which still gets the actionable message.
            if ioctl_unsupported(&e) {
                return e;
            }
            io::Error::new(
                e.kind(),
                format!(
                    "{}: compression failed ({e}); explicit levels require Linux 6.15+",
                    path.display()
                ),
            )
        })?;
        start += len;
        progress(start);
    }
    file.sync_all()
}
/// True when an ioctl failed because the filesystem or kernel does not provide
/// it, rather than because the request itself was rejected.
pub(crate) fn ioctl_unsupported(e: &io::Error) -> bool {
    // 25 is ENOTTY ("inappropriate ioctl for device"); EOPNOTSUPP maps to
    // ErrorKind::Unsupported. Stock/older kernels and non-Btrfs filesystems
    // (tmpfs test fixtures, ext4 CI runners) report one of these.
    e.raw_os_error() == Some(25) || e.kind() == io::ErrorKind::Unsupported
}
/// Best-effort Btrfs Zstd recompression for callers that have already produced
/// the desired bytes. The compression ioctl only exists on Btrfs with the
/// required kernel support; when it is missing the transformed file is still
/// installed rather than failing the whole pass. Real I/O failures still error.
pub(crate) fn compress_file_best_effort(file: &File, path: &Path, level: u8) -> io::Result<()> {
    match compress_file(file, path, level) {
        Ok(()) => Ok(()),
        Err(e) if ioctl_unsupported(&e) => {
            if env::var_os("BGC_VERBOSE").is_some() {
                eprintln!(
                    "{}: Btrfs compression unavailable on this filesystem; installed without recompressing",
                    path.display()
                );
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}
fn compress(tree: &Tree, level: u8) -> io::Result<()> {
    if !(1..=15).contains(&level) {
        return Err(invalid("ZSTD level must be 1..15"));
    }
    let mut count = 0u64;
    let mut processed = 0u64;
    let mut progress = Instant::now();
    tree.walk(|path, read| {
        let file = tree.open(path, true)?;
        let meta = file.metadata()?;
        if meta.ino() != read.metadata()?.ino() || !meta.is_file() {
            return Err(invalid("file changed during traversal"));
        }
        if env::var_os("BGC_VERBOSE").is_some() {
            eprintln!("Compressing {}", tree.display.join(path).display());
        }
        compress_file_progress(&file, path, level, |completed| {
            if progress.elapsed() >= Duration::from_secs(2) {
                eprintln!(
                    "Compression: {count} files complete, {} processed...",
                    human(processed + completed)
                );
                progress = Instant::now();
            }
        })?;
        count += 1;
        processed += meta.len();
        Ok(())
    })?;
    eprintln!("Compressed {count} files.");
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Extent {
    logical: u64,
    physical: u64,
    len: u64,
    reserved: [u64; 2],
    flags: u32,
    reserved2: [u32; 3],
}
#[repr(C)]
struct Fiemap {
    start: u64,
    len: u64,
    flags: u32,
    mapped: u32,
    count: u32,
    reserved: u32,
    extents: [Extent; 256],
}
fn fiemap(file: &File, mut visit: impl FnMut(Extent) -> io::Result<()>) -> io::Result<()> {
    let mut start = 0;
    loop {
        cancelled()?;
        let mut map = Fiemap {
            start,
            len: u64::MAX - start,
            flags: 1,
            mapped: 0,
            count: 256,
            reserved: 0,
            extents: [Extent::default(); 256],
        };
        call(file, request(3, b'f' as u64, 11, 32), &mut map)?;
        if map.mapped > 256 {
            return Err(invalid("invalid FIEMAP count"));
        }
        if map.mapped == 0 {
            break;
        }
        for ext in &map.extents[..map.mapped as usize] {
            visit(*ext)?;
        }
        let last = map.extents[map.mapped as usize - 1];
        let next = last
            .logical
            .checked_add(last.len)
            .ok_or_else(|| invalid("FIEMAP range overflow"))?;
        if last.flags & 1 != 0 {
            break;
        }
        if next <= start {
            return Err(invalid("FIEMAP did not advance"));
        }
        start = next;
    }
    Ok(())
}
fn usage(tree: &Tree) -> io::Result<()> {
    let (mut total, mut exclusive, mut shared) = (0u64, 0u64, 0u64);
    tree.walk(|_, f| {
        let size = f.metadata()?.len();
        fiemap(&f, |e| {
            let len = e.len.min(size.saturating_sub(e.logical));
            total += len;
            if e.flags & 0x2000 != 0 {
                shared += len;
            } else {
                exclusive += len;
            }
            Ok(())
        })
    })?;
    println!("{total}|{exclusive}|{shared}");
    Ok(())
}

#[repr(C)]
#[derive(Default)]
struct SearchKey {
    tree: u64,
    min_ino: u64,
    max_ino: u64,
    min_offset: u64,
    max_offset: u64,
    min_trans: u64,
    max_trans: u64,
    min_type: u32,
    max_type: u32,
    count: u32,
    pad: u32,
    unused: [u64; 4],
}
#[repr(C)]
struct Search {
    key: SearchKey,
    size: u64,
    buf: [u64; 8192],
}
#[derive(Default)]
struct Sizes {
    disk: u64,
    raw: u64,
    referenced: u64,
    seen: HashSet<u64>,
}
impl Sizes {
    fn add(&mut self, data: &[u8]) -> io::Result<()> {
        if data.len() < 21 {
            return Err(invalid("short extent item"));
        }
        let raw = le64(data, 8);
        match data[20] {
            0 => {
                self.disk += (data.len() - 21) as u64;
                self.raw += raw;
                self.referenced += raw;
            }
            1 | 2 => {
                if data.len() != 53 {
                    return Err(invalid("invalid regular extent item"));
                }
                let physical = le64(data, 21);
                if physical != 0 {
                    if self.seen.insert(physical) {
                        self.disk += le64(data, 29);
                        self.raw += raw;
                    }
                    self.referenced += le64(data, 45);
                }
            }
            _ => return Err(invalid("unknown extent type")),
        }
        Ok(())
    }
}
fn measure(tree: &Tree) -> io::Result<Sizes> {
    let mut sizes = Sizes::default();
    tree.walk(|_, file| add_file_sizes(&file, &mut sizes))?;
    Ok(sizes)
}
fn add_file_sizes(file: &File, sizes: &mut Sizes) -> io::Result<()> {
        file.sync_all()?;
        let ino = file.metadata()?.ino();
        let mut search = Search {
            key: SearchKey {
                min_ino: ino,
                max_ino: ino,
                max_offset: u64::MAX,
                max_trans: u64::MAX,
                min_type: 108,
                max_type: 108,
                ..Default::default()
            },
            size: 65536,
            buf: [0; 8192],
        };
        loop {
            cancelled()?;
            search.key.count = u32::MAX;
            search.size = 65536;
            call(&file, request(3, 0x94, 17, 112), &mut search)?;
            if search.key.count == 0 {
                break;
            }
            let buf = bytes(&search.buf);
            let mut pos = 0;
            let mut last = 0;
            for _ in 0..search.key.count {
                if pos + 32 > buf.len() {
                    return Err(invalid("truncated tree search header"));
                }
                let len = ne32(buf, pos + 28) as usize;
                last = ne64(buf, pos + 16);
                if ne64(buf, pos + 8) != ino
                    || ne32(buf, pos + 24) != 108
                    || last < search.key.min_offset
                    || pos + 32 + len > buf.len()
                {
                    return Err(invalid("invalid tree search result"));
                }
                pos += 32;
                sizes.add(&buf[pos..pos + len])?;
                pos += len;
            }
            if last == u64::MAX {
                break;
            }
            search.key.min_offset = last + 1;
        }
        Ok(())
}
fn human(n: u64) -> String {
    format!("{:.3}M", n as f64 / 1048576.0)
}

// Sort fixed-size hash records on disk, in bounded batches. No database runtime,
// shell sort, or RAM proportional to the game size is required.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Record {
    hash: u64,
    len: u64,
    file: u64,
    offset: u64,
}
impl Record {
    fn write(&self, w: &mut impl Write) -> io::Result<()> {
        for n in [self.hash, self.len, self.file, self.offset] {
            w.write_all(&n.to_le_bytes())?;
        }
        Ok(())
    }
    fn read(r: &mut impl Read) -> io::Result<Option<Self>> {
        let mut b = [0u8; 32];
        if r.read(&mut b[..1])? == 0 {
            return Ok(None);
        }
        r.read_exact(&mut b[1..])?;
        Ok(Some(Self {
            hash: le64(&b, 0),
            len: le64(&b, 8),
            file: le64(&b, 16),
            offset: le64(&b, 24),
        }))
    }
}
struct Scratch {
    dir: PathBuf,
    next: usize,
}
impl Scratch {
    fn new(parent: &Path) -> io::Result<Self> {
        fs::create_dir_all(parent)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("clock before epoch"))?
            .as_nanos();
        let dir = parent.join(format!("native-{}-{stamp}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self { dir, next: 0 })
    }
    fn path(&mut self) -> PathBuf {
        self.next += 1;
        self.dir.join(self.next.to_string())
    }
    fn flush(&mut self, records: &mut Vec<Record>) -> io::Result<PathBuf> {
        records.sort_unstable();
        let path = self.path();
        let mut out = BufWriter::new(File::create(&path)?);
        for r in records.drain(..) {
            r.write(&mut out)?;
        }
        out.flush()?;
        Ok(path)
    }
}
use std::os::unix::fs::DirBuilderExt;
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn merge(paths: &[PathBuf], mut visit: impl FnMut(Record) -> io::Result<()>) -> io::Result<()> {
    let mut readers = paths
        .iter()
        .map(|p| File::open(p).map(BufReader::new))
        .collect::<io::Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::new();
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some(rec) = Record::read(r)? {
            heap.push(Reverse((rec, i)));
        }
    }
    while let Some(Reverse((rec, i))) = heap.pop() {
        cancelled()?;
        visit(rec)?;
        if let Some(next) = Record::read(&mut readers[i])? {
            heap.push(Reverse((next, i)));
        }
    }
    Ok(())
}
fn compact(scratch: &mut Scratch, mut runs: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    while runs.len() > 64 {
        let mut next = Vec::new();
        for chunk in runs.chunks(64) {
            let path = scratch.path();
            let mut out = BufWriter::new(File::create(&path)?);
            merge(chunk, |r| r.write(&mut out))?;
            out.flush()?;
            for old in chunk {
                fs::remove_file(old)?;
            }
            next.push(path);
        }
        runs = next;
    }
    Ok(runs)
}
#[repr(C)]
#[derive(Default)]
struct Dedupe {
    offset: u64,
    len: u64,
    count: u16,
    reserved: u16,
    reserved2: u32,
    dest_fd: i64,
    dest_offset: u64,
    bytes: u64,
    status: i32,
    pad: u32,
}
fn share(src: &File, dst: &File, from: u64, to: u64, len: u64) -> io::Result<u64> {
    // Clamp to both current file sizes. Never round a range past EOF.
    let length = len
        .min(src.metadata()?.len().saturating_sub(from))
        .min(dst.metadata()?.len().saturating_sub(to));
    if length != len {
        return Err(invalid("file shrank during deduplication"));
    }
    let mut args = Dedupe {
        offset: from,
        len,
        count: 1,
        dest_fd: dst.as_raw_fd() as i64,
        dest_offset: to,
        ..Default::default()
    };
    call(src, request(3, 0x94, 54, 24), &mut args)?;
    match args.status {
        0 => Ok(args.bytes),
        1 => Ok(0),
        s if s < 0 => Err(io::Error::from_raw_os_error(-s)),
        _ => Err(invalid("unexpected dedupe status")),
    }
}
struct Indexed {
    path: PathBuf,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
}
impl Indexed {
    fn new(path: &Path, m: &Metadata) -> Self {
        Self {
            path: path.to_owned(),
            ino: m.ino(),
            size: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
        }
    }
    fn open(&self, tree: &Tree) -> io::Result<File> {
        let file = tree.open(&self.path, true)?;
        let m = file.metadata()?;
        if !m.is_file()
            || m.ino() != self.ino
            || m.len() != self.size
            || m.mtime() != self.mtime
            || m.mtime_nsec() != self.mtime_ns
        {
            return Err(invalid("file changed since hashing"));
        }
        Ok(file)
    }
}
fn dedupe(tree: &Tree, parent: &Path) -> io::Result<()> {
    fs::create_dir_all(parent)?;
    if fs::canonicalize(parent)?.starts_with(fs::canonicalize(&tree.display)?) {
        return Err(invalid(
            "scratch directory must be outside the scanned tree",
        ));
    }
    let mut scratch = Scratch::new(parent)?;
    let mut files = Vec::new();
    let mut records = Vec::with_capacity(262144);
    let mut runs = Vec::new();
    let block = tree.sector as usize;
    let mut data = vec![0u8; block];
    let mut hashed = 0u64;
    tree.walk(|path, file| {
        let meta = file.metadata()?;
        let id = files.len() as u64;
        files.push(Indexed::new(path, &meta));
        if env::var_os("BGC_VERBOSE").is_some() {
            eprintln!("Hashing {}", tree.display.join(path).display());
        }
        // FIEMAP avoids materialising holes or deduping unwritten allocations.
        fiemap(&file, |ext| {
            if ext.flags & (0x800 | 0x200) != 0 {
                return Ok(());
            } // inline, preallocated
            let begin = ext.logical.div_ceil(tree.sector) * tree.sector;
            let end =
                ext.logical.saturating_add(ext.len).min(meta.len()) / tree.sector * tree.sector;
            for offset in (begin..end).step_by(block) {
                cancelled()?;
                file.read_exact_at(&mut data, offset)?;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut h);
                records.push(Record {
                    hash: h.finish(),
                    len: tree.sector,
                    file: id,
                    offset,
                });
                hashed += tree.sector;
                if hashed.is_multiple_of(1024 * 1024 * 1024) {
                    eprintln!("Hashed {} GiB...", hashed / (1024 * 1024 * 1024));
                }
                if records.len() == 262144 {
                    runs.push(scratch.flush(&mut records)?);
                }
            }
            Ok(())
        })
    })?;
    if !records.is_empty() {
        runs.push(scratch.flush(&mut records)?);
    }
    let runs = compact(&mut scratch, runs)?;
    eprintln!("Comparing duplicate data in {} files...", files.len());
    let mut source: Option<Record> = None;
    let mut source_fd: Option<File> = None;
    let (mut matched, mut errors, mut candidates) = (0u64, 0u64, 0u64);
    merge(&runs, |rec| {
        if let Some(ref first) = source {
            if (first.hash, first.len) == (rec.hash, rec.len) {
                candidates += 1;
                if source_fd.is_none() {
                    source_fd = Some(files[first.file as usize].open(tree)?);
                }
                let result = files[rec.file as usize].open(tree).and_then(|dst| {
                    share(
                        source_fd.as_ref().unwrap(),
                        &dst,
                        first.offset,
                        rec.offset,
                        rec.len,
                    )
                });
                match result {
                    Ok(n) => matched += n,
                    Err(e) => {
                        errors += 1;
                        if errors <= 20 {
                            eprintln!(
                                "Cannot share {} at {}: {e}",
                                files[rec.file as usize].path.display(),
                                rec.offset
                            );
                        }
                    }
                }
                if candidates.is_multiple_of(10000)
                    && (env::var_os("BGC_VERBOSE").is_some()
                        || candidates.is_multiple_of(1_000_000))
                {
                    eprintln!("Checked {candidates} duplicate ranges; {matched} bytes accepted.");
                }
                return Ok(());
            }
        }
        source = Some(rec);
        source_fd = None;
        Ok(())
    })?;
    if env::var_os("BGC_VERBOSE").is_some() {
        println!("Deduplication: {candidates} duplicate ranges, {matched} logical bytes accepted, {errors} rejected. Accepted bytes include already-shared data; they are not disk savings.");
    } else {
        println!("Deduplication: {candidates} candidate ranges checked, {errors} rejected.");
    }
    if errors != 0 {
        return Err(invalid(
            "deduplication incomplete; see rejected ranges above",
        ));
    }
    Ok(())
}
fn balance(tree: &Tree) -> io::Result<()> {
    let mut args = [0u64; 128];
    args[0] = 4; // BTRFS_BALANCE_METADATA (not SYSTEM)
    call(&tree.root, request(3, 0x94, 32, 1024), &mut args)
}
fn run() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--protocol-version" {
        println!("1");
        return Ok(());
    }
    if args.len() == 1 && args[0] == "--version" {
        println!("bgc-native {} protocol 1", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() == 1 && args[0] == "--licenses" {
        print!("{}", include_str!("../../THIRD_PARTY.md"));
        return Ok(());
    }
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("");
    if (command == "prune-plan" || command == "prune-apply") && args.len() >= 2 {
        let (debug, rels) = prune_args(&args[2..])?;
        return if command == "prune-plan" {
            assets::prune_plan(Path::new(&args[1]), &rels, debug)
        } else {
            assets::prune_apply(Path::new(&args[1]), &rels, debug)
        };
    }
    if command == "assets-physical-rejections" && args.len() == 2 {
        return assets::physical_rejections(Path::new(&args[1]));
    }
    if command == "assets-restore-file" && args.len() == 3 {
        return assets::restore_file(Path::new(&args[1]),Path::new(&args[2]));
    }
    if command == "measure-file" && args.len() == 2 {
        let path = Path::new(&args[1]);
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = path.file_name().ok_or_else(|| invalid("expected a regular file path"))?;
        let tree = Tree::new(parent)?;
        let file = tree.open(Path::new(name), false)?;
        if !file.metadata()?.is_file() { return Err(invalid("expected a regular file")); }
        let mut sizes = Sizes::default();
        add_file_sizes(&file, &mut sizes)?;
        println!("{}|{}|{}", sizes.disk, sizes.raw, sizes.referenced);
        return Ok(());
    }
    if command == "package-audit" && args.len() == 2 {
        return packages::audit(Path::new(&args[1]));
    }
    if command == "variants-scan" && args.len() == 2 {
        return variants::scan(Path::new(&args[1]));
    }
    if command == "engine-scan" && args.len() == 2 {
        return engines::scan(Path::new(&args[1]));
    }
    if command == "unityfs-audit" && args.len() == 2 {
        return unityfs::run(Path::new(&args[1]), None, 5.0);
    }
    if command == "godot-audit" && args.len() == 2 {
        return containers::godot_audit(Path::new(&args[1]));
    }
    if command == "container-audit" && args.len() == 2 {
        return containers::audit(Path::new(&args[1]));
    }
    if command == "godot-dedup-audit" && args.len() == 2 {
        return containers::godot_dedup(Path::new(&args[1]), None, 5.0);
    }
    if command == "godot-dedup-export" && args.len() == 4 {
        let pct = args[1].to_str().ok_or_else(|| invalid("invalid percentage encoding"))?
            .parse::<f64>().map_err(|_| invalid("expected write-efficiency percentage"))?;
        return containers::godot_dedup(Path::new(&args[2]), Some(Path::new(&args[3])), pct);
    }
    if command == "godot-texture-audit" && args.len() == 3 {
        return containers::godot_texture_transform(Path::new(&args[2]), None, 0.0, args[1].to_str().unwrap_or(""));
    }
    if command == "godot-texture-export" && args.len() == 5 {
        let pct = args[2].to_str().ok_or_else(|| invalid("invalid percentage encoding"))?
            .parse::<f64>().map_err(|_| invalid("expected write-efficiency percentage"))?;
        return containers::godot_texture_transform(Path::new(&args[3]), Some(Path::new(&args[4])), pct, args[1].to_str().unwrap_or(""));
    }
    if command == "godot3-audit" && args.len() == 3 {
        return containers::godot3_optimize(Path::new(&args[2]), None, 0.0, args[1].to_str().unwrap_or(""));
    }
    if command == "godot3-optimize" && args.len() == 5 {
        let pct = args[2].to_str().ok_or_else(|| invalid("invalid percentage encoding"))?
            .parse::<f64>().map_err(|_| invalid("expected write-efficiency percentage"))?;
        return containers::godot3_optimize(Path::new(&args[3]), Some(Path::new(&args[4])), pct, args[1].to_str().unwrap_or(""));
    }
    if command == "godot3-verify" && args.len() == 3 {
        return containers::godot3_verify(Path::new(&args[1]), Path::new(&args[2]));
    }
    if command == "unreal-audit" && args.len() == 2 {
        return containers::unreal_audit(Path::new(&args[1]));
    }
    if command == "unityfs-recompress" && args.len() == 4 {
        let pct = args[1].to_str().ok_or_else(|| invalid("invalid percentage encoding"))?
            .parse::<f64>().map_err(|_| invalid("expected write-efficiency percentage"))?;
        return unityfs::run(Path::new(&args[2]), Some(Path::new(&args[3])), pct);
    }
    if command == "variants-slim" && (args.len() == 2 || args.len() == 3) {
        let keep: Vec<String> = if args.len() == 3 {
            args[2]
                .to_str()
                .unwrap_or("")
                .split(',')
                .map(variants::canonical_language)
                .filter(|code| !code.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        return variants::slim(Path::new(&args[1]), &keep);
    }
    if command == "fmod-audit" && args.len() == 2 {
        return fmod::audit(Path::new(&args[1]));
    }
    if command == "fmod-reencode" && args.len() == 4 {
        let profile = args[1].to_str().ok_or_else(|| invalid("invalid FMOD profile"))?;
        return fmod::run(profile,Path::new(&args[2]),Path::new(&args[3]));
    }
    if command == "assets" && args.len() == 5 {
        let action = args[1]
            .to_str()
            .ok_or_else(|| invalid("invalid asset action"))?;
        let mode = args[2]
            .to_str()
            .ok_or_else(|| invalid("invalid visual target"))?;
        let level = args[3]
            .to_str()
            .and_then(|s| s.parse::<u8>().ok())
            .ok_or_else(|| invalid("invalid asset compression level"))?;
        return assets::run(action, mode, level, Path::new(&args[4]));
    }
    let (root,level)=match (command,args.len()) {
        ("compress",3)=>(Path::new(&args[2]),args[1].to_str().and_then(|s|s.parse::<u8>().ok()).ok_or_else(||invalid("invalid level"))?),
        ("measure"|"measure-bytes"|"usage"|"balance",2)=>(Path::new(&args[1]),0),
        ("dedupe",3)=>(Path::new(&args[1]),0),
        _=>return Err(invalid("usage: bgc-native compress LEVEL DIR | measure DIR | measure-bytes DIR | measure-file FILE | usage DIR | dedupe DIR SCRATCH_DIR | balance DIR | assets ACTION TARGET LEVEL DIR | assets-restore-file DIR RELATIVE_PATH | assets-physical-rejections DIR | prune-plan DIR [--debug] [REL...] | prune-apply DIR [--debug] [REL...] | fmod-reencode PROFILE INPUT OUTPUT | fmod-audit INPUT | package-audit INPUT | engine-scan DIR | container-audit FILE | unityfs-audit FILE | unityfs-recompress MIN_PCT INPUT OUTPUT | godot-audit FILE | godot-dedup-audit FILE | godot-dedup-export MIN_PCT INPUT OUTPUT | godot-texture-audit PROFILE FILE | godot-texture-export PROFILE MIN_PCT INPUT OUTPUT | godot3-audit PROFILE FILE | godot3-optimize PROFILE MIN_PCT INPUT OUTPUT | unreal-audit FILE | --licenses")),
    };
    let tree = Tree::new(root)?;
    match command {
        "compress" => compress(&tree, level),
        "usage" => usage(&tree),
        "dedupe" => dedupe(&tree, Path::new(&args[2])),
        "balance" => balance(&tree),
        "measure" => {
            let s = measure(&tree)?;
            let ratio = if s.raw == 0 {
                0.0
            } else {
                100.0 * s.disk as f64 / s.raw as f64
            };
            println!(
                "TOTAL {ratio:.1}% {} {} {} {} {} {}",
                human(s.disk),
                human(s.raw),
                human(s.referenced),
                s.disk,
                s.raw,
                s.referenced
            );
            Ok(())
        }
        "measure-bytes" => {
            let s = measure(&tree)?;
            println!("{}|{}|{}", s.disk, s.raw, s.referenced);
            Ok(())
        }
        _ => unreachable!(),
    }
}
fn main() {
    unsafe {
        signal(2, stop as *const () as usize);
        signal(15, stop as *const () as usize);
    }
    let result = run();
    let sig = SIGNAL.load(Ordering::Relaxed);
    if let Err(e) = result {
        eprintln!("bgc-native: {e}");
        std::process::exit(if sig != 0 { 128 + sig as i32 } else { 1 });
    }
    if sig != 0 {
        std::process::exit(128 + sig as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_layouts() {
        assert_eq!(std::mem::size_of::<Defrag>(), 48);
        assert_eq!(std::mem::size_of::<Dedupe>(), 56);
        assert_eq!(std::mem::size_of::<SearchKey>(), 104);
        assert_eq!(std::mem::size_of::<Extent>(), 56);
        assert_eq!(request(1, 0x94, 16, 48), 0x40309410);
        assert_eq!(request(3, 0x94, 54, 24), 0xc0189436);
    }
    #[test]
    fn truncated_record_is_error() {
        assert!(Record::read(&mut &b"abc"[..]).is_err());
        assert!(Record::read(&mut &b""[..]).unwrap().is_none());
    }
    #[test]
    fn record_roundtrip() {
        let r = Record {
            hash: 9,
            len: 4096,
            file: 3,
            offset: 8192,
        };
        let mut b = Vec::new();
        r.write(&mut b).unwrap();
        assert_eq!(Record::read(&mut &b[..]).unwrap(), Some(r));
    }
    #[test]
    fn extent_accounting() {
        let mut s = Sizes::default();
        let mut b = [0u8; 53];
        b[8..16].copy_from_slice(&8192u64.to_le_bytes());
        b[20] = 1;
        b[21..29].copy_from_slice(&65536u64.to_le_bytes());
        b[29..37].copy_from_slice(&4096u64.to_le_bytes());
        b[45..53].copy_from_slice(&8192u64.to_le_bytes());
        s.add(&b).unwrap();
        s.add(&b).unwrap();
        assert_eq!((s.disk, s.raw, s.referenced), (4096, 8192, 16384));
        b[21..29].fill(0);
        s.add(&b).unwrap();
        assert_eq!(s.disk, 4096);
        assert!(s.add(&b[..20]).is_err());
    }
    #[test]
    fn sorted_spill_merge() {
        let mut scratch = Scratch::new(&env::temp_dir()).unwrap();
        let mut runs = Vec::new();
        for n in (0..70).rev() {
            let mut batch = vec![Record {
                hash: n,
                len: 4096,
                file: 0,
                offset: 0,
            }];
            runs.push(scratch.flush(&mut batch).unwrap());
        }
        let runs = compact(&mut scratch, runs).unwrap();
        let mut got = Vec::new();
        merge(&runs, |r| {
            got.push(r.hash);
            Ok(())
        })
        .unwrap();
        assert_eq!(got, (0..70).collect::<Vec<_>>());
    }
}
