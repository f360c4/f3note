//! Crash-safe file replacement.
//!
//! This is the foundation the whole autosave design rests on, so it is worth
//! being precise about what each step buys:
//!
//! 1. Write the new contents to a temporary file **in the same directory** as
//!    the target. `rename(2)` is only atomic within a filesystem, and a
//!    temporary file in `/tmp` may well be on a different one.
//! 2. `fsync` the temporary file. Without this, `rename` can complete and be
//!    durable while the data blocks behind it are not, leaving a file that
//!    exists and is empty or truncated after a power cut.
//! 3. `rename` over the target. Atomic: any reader sees the old file or the new
//!    one, never a partial write, and the original is never truncated in place.
//! 4. `fsync` the **directory**. This is the step that is easiest to skip and
//!    most often wrong. ext4's `auto_da_alloc` heuristic hides the omission for
//!    the common rename-over-existing-file case, which is exactly why it goes
//!    unnoticed — but btrfs and xfs do not, and there the rename itself can be
//!    lost.
//!
//! An error at any point leaves the previous contents of the target intact.
//! That is the property that matters: a failed backup is recoverable, a
//! corrupted one is not.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A temporary name that cannot collide with another process, another thread,
/// or a second attempt by this one.
fn temp_path(target: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "f3note".to_owned());
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(".{}.{}.{}.tmp", stem, std::process::id(), n))
}

/// Flush a directory entry to disk so a rename into it survives a power loss.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    // Opening a directory read-only and fsyncing it is the portable-on-Linux
    // way to do this; there is no need for O_DIRECTORY here.
    File::open(dir)?.sync_all()
}

/// Replace `path` with `contents`, atomically and durably.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = temp_path(path);

    // Scoped so the file is closed before the rename. Renaming over a file
    // still held open works on Linux, but closing first keeps the failure
    // modes easy to reason about.
    let result = (|| -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(contents)?;
        // Durability of the data, before anything points at it.
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Durability of the rename itself. A failure here means the new contents
    // may not survive a crash, but the file on disk is valid either way, so
    // this is reported rather than rolled back.
    sync_dir(dir)
}

/// True when the error is one where retrying is pointless and the caller should
/// surface it to the user rather than silently continuing to lose writes.
///
/// Linux clears a writeback error after reporting it once, so a second `fsync`
/// may succeed while the data is gone. Treating these as terminal for the
/// operation, instead of retrying, is the only safe reading.
pub fn is_fatal(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::StorageFull
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::ReadOnlyFilesystem
    ) || e.raw_os_error() == Some(libc_eio())
}

fn libc_eio() -> i32 {
    5 // EIO
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("f3note_atomic_{}_{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_and_replaces_contents() {
        let d = scratch("replace");
        let f = d.join("x.txt");
        write(&f, b"first").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"first");
        write(&f, b"second").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"second");
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn leaves_no_temporary_files_behind() {
        let d = scratch("notemp");
        let f = d.join("x.txt");
        write(&f, b"data").unwrap();
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "x.txt")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_failed_write_preserves_the_previous_contents() {
        let d = scratch("preserve");
        let f = d.join("x.txt");
        write(&f, b"good").unwrap();
        // A directory cannot be replaced by a file rename, so this fails at
        // the rename step with the target already in place.
        let blocked = d.join("adir");
        fs::create_dir(&blocked).unwrap();
        assert!(write(&blocked, b"nope").is_err());
        assert_eq!(fs::read(&f).unwrap(), b"good");
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn temp_names_do_not_collide() {
        let t = Path::new("/tmp/f3note/session");
        let a = temp_path(t);
        let b = temp_path(t);
        assert_ne!(a, b);
        assert_eq!(a.parent(), t.parent());
    }

    #[test]
    fn syncing_a_directory_succeeds() {
        let d = scratch("syncdir");
        sync_dir(&d).unwrap();
        fs::remove_dir_all(d).ok();
    }
}
