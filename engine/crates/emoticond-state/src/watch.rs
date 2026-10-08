//! Reload-on-change by polling.
//!
//! No inotify and no threads: before a search a front-end calls
//! [`Watched::refresh`], which re-stats the files at most once per interval
//! and reloads only if one of them changed (mtime, size or inode). Writers
//! replace files by rename, so a new inode is a reliable change signal even
//! when mtime and size happen to match.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Default re-stat interval: once per second.
pub const DEFAULT_INTERVAL_MS: u64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
    ino: u64,
}

fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let ino = 0;
    Some(Stamp { mtime: m.modified().ok(), len: m.len(), ino })
}

type Loader<T> = Box<dyn Fn(&[PathBuf]) -> T + Send + Sync>;

/// A value loaded from one or more files, reloaded when they change.
/// A missing file is a state too (it can appear or disappear).
pub struct Watched<T> {
    paths: Vec<PathBuf>,
    load: Loader<T>,
    value: T,
    stamps: Vec<Option<Stamp>>,
    interval_ms: u64,
    last_check: Option<u64>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Watched<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watched").field("paths", &self.paths).field("value", &self.value).finish_non_exhaustive()
    }
}

impl<T> Watched<T> {
    /// Load now from `paths` with `load`, and re-stat at most every
    /// `interval_ms` on [`refresh`](Self::refresh).
    pub fn new(
        paths: Vec<PathBuf>,
        interval_ms: u64,
        load: impl Fn(&[PathBuf]) -> T + Send + Sync + 'static,
    ) -> Watched<T> {
        let stamps = paths.iter().map(|p| stamp(p)).collect();
        let value = load(&paths);
        Watched { paths, load: Box::new(load), value, stamps, interval_ms, last_check: None }
    }

    /// One file.
    pub fn file(
        path: impl Into<PathBuf>,
        interval_ms: u64,
        load: impl Fn(&Path) -> T + Send + Sync + 'static,
    ) -> Watched<T> {
        Watched::new(vec![path.into()], interval_ms, move |ps| load(&ps[0]))
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Re-stat the files if `interval_ms` has passed since the last check (or
    /// the clock went back), and reload if any changed. Returns whether the
    /// value was reloaded.
    pub fn refresh(&mut self, now: u64) -> bool {
        if let Some(last) = self.last_check {
            if now >= last && now - last < self.interval_ms {
                return false;
            }
        }
        self.last_check = Some(now);
        self.reload_if_changed()
    }

    /// Re-stat now, ignoring the interval; reload if anything changed.
    pub fn reload_if_changed(&mut self) -> bool {
        let now: Vec<Option<Stamp>> = self.paths.iter().map(|p| stamp(p)).collect();
        if now == self.stamps {
            return false;
        }
        self.stamps = now;
        self.value = (self.load)(&self.paths);
        true
    }

    /// Reload unconditionally.
    pub fn reload(&mut self) {
        self.stamps = self.paths.iter().map(|p| stamp(p)).collect();
        self.value = (self.load)(&self.paths);
    }

    /// Replace the value after this process wrote the files itself, so the
    /// next refresh does not reload what we already hold.
    pub fn set_written(&mut self, value: T) {
        self.stamps = self.paths.iter().map(|p| stamp(p)).collect();
        self.value = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn reloads_on_change_at_most_once_per_interval() {
        let d = TempDir::new();
        let p = d.path().join("f.txt");
        let mut w = Watched::file(p.clone(), 1000, |p| std::fs::read_to_string(p).unwrap_or_default());
        assert_eq!(w.get(), "");
        assert!(!w.refresh(10_000));
        crate::fsutil::write_atomic(&p, b"a").unwrap();
        // within the interval: not even stat'ed
        assert!(!w.refresh(10_500));
        assert_eq!(w.get(), "");
        assert!(w.refresh(11_000));
        assert_eq!(w.get(), "a");
        assert!(!w.refresh(12_000));
        // same size, new inode (rename) is still a change
        crate::fsutil::write_atomic(&p, b"b").unwrap();
        assert!(w.refresh(13_000));
        assert_eq!(w.get(), "b");
        std::fs::remove_file(&p).unwrap();
        assert!(w.refresh(14_000));
        assert_eq!(w.get(), "");
    }

    #[test]
    fn set_written_avoids_a_reload() {
        let d = TempDir::new();
        let p = d.path().join("f.txt");
        let mut w = Watched::file(p.clone(), 0, |p| std::fs::read_to_string(p).unwrap_or_default());
        crate::fsutil::write_atomic(&p, b"mine").unwrap();
        w.set_written("mine".into());
        assert!(!w.refresh(1));
    }
}
