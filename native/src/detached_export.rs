//! Development-only publication of an already-verified detached artifact.
//! Anonymous staging avoids partial destination files and pathname cleanup races.
//! No replacement, installed apply, fallback to named staging, or source writes.
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{self, Write},
    os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::OpenOptionsExt}},
    path::{Component, Path},
};

/// Publish complete bytes under a new name in a held destination directory.
/// `before_publish` runs after staging/sync, immediately before the commit point.
/// The initial parent lookup is NOT a trusted-root sandbox; callers must choose
/// a trusted research directory. Directory relocation is not prevented.
pub fn publish(output: &Path, bytes: &[u8], before_publish: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    super::cancelled()?;
    // Reject trailing separators, dot/parent components and paths without a
    // basename, rather than publishing under a normalized, surprising name.
    let raw = output.as_os_str().as_bytes();
    let tail = raw.rsplit(|&b| b == b'/').next().unwrap_or_default();
    if tail.is_empty() || tail == b"." || tail == b".." {
        return Err(super::invalid("detached export requires a destination basename"));
    }
    let Some(Component::Normal(name)) = output.components().next_back() else {
        return Err(super::invalid("invalid detached export destination"));
    };
    let name = CString::new(name.as_bytes()).map_err(|_| super::invalid("export destination contains NUL"))?;
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let directory = OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(parent)?;
    let dot = c".";
    // O_EXCL must NOT be combined with O_TMPFILE: it would forbid linking the
    // staged inode. A filesystem lacking O_TMPFILE fails closed, without output.
    let fd = unsafe { libc::openat(directory.as_raw_fd(), dot.as_ptr(),
        libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC, 0o600 as libc::mode_t) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    let mut staged = unsafe { File::from_raw_fd(fd) };
    for chunk in bytes.chunks(1024 * 1024) {
        super::cancelled()?;
        staged.write_all(chunk)?;
    }
    staged.sync_all()?;
    before_publish()?;
    super::cancelled()?;
    // /proc/self/fd names our held anonymous inode, not a writable temporary
    // pathname. This documented O_TMPFILE linking method needs no CAP_DAC_READ_SEARCH
    // (unlike AT_EMPTY_PATH). Missing /proc support fails closed.
    let source = CString::new(format!("/proc/self/fd/{}", staged.as_raw_fd())).unwrap();
    let linked = unsafe { libc::linkat(libc::AT_FDCWD, source.as_ptr(),
        directory.as_raw_fd(), name.as_ptr(), libc::AT_SYMLINK_FOLLOW) };
    if linked != 0 { return Err(io::Error::last_os_error()); }
    // Commit point passed: cancellation must not turn a completed publication
    // into an apparent precommit failure. Never unlink output on a later error.
    directory.sync_all().map_err(|error| io::Error::new(error.kind(),
        format!("detached export published completely, but directory durability is unconfirmed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::{symlink, PermissionsExt}, time::{SystemTime, UNIX_EPOCH}};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("bgc-detached-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    #[test]
    fn publishes_complete_private_artifact_without_replacing_existing_names() {
        let fixture = Fixture::new();
        let output = fixture.0.join("artifact.xnb");
        publish(&output, b"complete", || {
            assert!(!output.exists());
            assert_eq!(fs::read_dir(&fixture.0)?.count(), 0);
            Ok(())
        }).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"complete");
        assert_eq!(fs::metadata(&output).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(publish(&output, b"replacement", || Ok(())).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&output).unwrap(), b"complete");
        let link = fixture.0.join("link.xnb");
        symlink("artifact.xnb", &link).unwrap();
        assert!(publish(&link, b"replacement", || Ok(())).is_err());
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn abort_and_destination_race_leave_no_partial_or_removed_files() {
        let fixture = Fixture::new();
        let output = fixture.0.join("artifact.xnb");
        assert!(publish(&output, b"candidate", || Err(super::super::invalid("source changed"))).is_err());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
        let result = publish(&output, b"candidate", || fs::write(&output, b"other writer"));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&output).unwrap(), b"other writer");
        for suffix in ["/", "/.", "/.."] {
            assert!(publish(Path::new(&format!("{}{suffix}", fixture.0.display())), b"candidate", || Ok(())).is_err());
        }
        let linked_parent = fixture.0.join("linked-parent");
        symlink(&fixture.0, &linked_parent).unwrap();
        assert!(publish(&linked_parent.join("other.xnb"), b"candidate", || Ok(())).is_err());
        assert!(!fixture.0.join("other.xnb").exists());
    }
}
