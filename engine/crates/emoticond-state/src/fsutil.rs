//! Atomic writes, locked appends and the sibling lock file
//!.
//!
//! Locks are advisory (`std::fs::File::lock`, flock on Unix, LockFileEx on
//! Windows) and taken on `<file>.lock`, never on the data file itself: the
//! data file is replaced by rename, so a lock on it would be on the old
//! inode.

use crate::error::{Error, Result};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Holds the exclusive lock until dropped (closing the file releases it).
#[must_use = "the lock is released when the guard is dropped"]
pub(crate) struct LockGuard {
    _file: File,
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub(crate) fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    Ok(())
}

/// Take the exclusive writer lock for `path` (blocking).
pub(crate) fn lock(path: &Path) -> Result<LockGuard> {
    ensure_parent(path)?;
    let lp = sibling(path, ".lock");
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lp)
        .map_err(|e| Error::io(&lp, e))?;
    f.lock().map_err(|e| Error::io(&lp, e))?;
    Ok(LockGuard { _file: f })
}

/// The file's bytes, or `None` if it does not exist.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// Replace `path` with `bytes`: temp file in the same dir, fsync, rename,
/// then fsync the dir (best effort). The caller holds the lock.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static N: AtomicU64 = AtomicU64::new(0);
    ensure_parent(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Append one line (a `\n` is added) in a single write. The caller holds
/// the lock. `sync` also fsyncs the data.
pub(crate) fn append_line(path: &Path, line: &str, sync: bool) -> Result<()> {
    ensure_parent(path)?;
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = OpenOptions::new().create(true).append(true).open(path).map_err(|e| Error::io(path, e))?;
    f.write_all(buf.as_bytes()).map_err(|e| Error::io(path, e))?;
    if sync {
        f.sync_data().map_err(|e| Error::io(path, e))?;
    }
    Ok(())
}

/// Serialise to one JSON line.
pub(crate) fn json_line<T: serde::Serialize>(path: &Path, v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Json { path: path.to_path_buf(), source: e })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let d = TempDir::new();
        let p = d.path().join("sub/x.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<String> = std::fs::read_dir(d.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["x.json"]);
    }

    #[test]
    fn read_optional_missing_is_none() {
        let d = TempDir::new();
        assert!(read_optional(&d.path().join("nope")).unwrap().is_none());
    }
}
