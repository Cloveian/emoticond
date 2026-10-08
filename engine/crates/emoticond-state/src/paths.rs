//! Where the state files live (docs/options.md §7.1).
//!
//! `$XDG_STATE_HOME/emoticond/`, falling back to `~/.local/state/emoticond/`.
//! `emoticond-config` computes the real locations (platform dirs elsewhere)
//! and passes the root with [`StatePaths::at`]; tests do the same with a
//! temp dir. This crate never reads `/etc` policy itself.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// The app dir name under each XDG base dir.
pub const APP_DIR: &str = "emoticond";

/// Every file this crate writes, under one root.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StatePaths {
    /// `$XDG_STATE_HOME/emoticond`
    pub root: PathBuf,
    /// `usage.json`: aggregated, decayed popularity.
    pub usage: PathBuf,
    /// `blocklist.txt`: faces hidden by offensive reports (and the user).
    pub blocklist: PathBuf,
    /// `reports/queue.jsonl`: reports waiting to be sent.
    pub queue: PathBuf,
    /// `reports/sent/`: what has been delivered.
    pub sent_dir: PathBuf,
    /// `reports/sent/sent.jsonl`: one line per delivered report (id, key).
    pub sent_log: PathBuf,
    /// `overlays/`: machine-written overlays (read after shipped data,
    /// before the hand-written ones in the config dir).
    pub overlays: PathBuf,
    /// `overlays/boosts.jsonl`: negative boosts from "doesn't fit".
    pub boosts: PathBuf,
    /// `outbox/`: shared-mode usage batches waiting to be sent.
    pub outbox: PathBuf,
    /// `devlog/`: a default home for the dev logs when the caller wants one
    /// in state (a dev setup can point them elsewhere).
    pub devlog: PathBuf,
}

impl StatePaths {
    /// All paths under an explicit root.
    pub fn at(root: impl Into<PathBuf>) -> StatePaths {
        let root = root.into();
        let reports = root.join("reports");
        let sent_dir = reports.join("sent");
        let overlays = root.join("overlays");
        StatePaths {
            usage: root.join("usage.json"),
            blocklist: root.join("blocklist.txt"),
            queue: reports.join("queue.jsonl"),
            sent_log: sent_dir.join("sent.jsonl"),
            sent_dir,
            boosts: overlays.join("boosts.jsonl"),
            overlays,
            outbox: root.join("outbox"),
            devlog: root.join("devlog"),
            root,
        }
    }

    /// The default root from the environment: `$XDG_STATE_HOME/emoticond`, or
    /// `$HOME/.local/state/emoticond`. Relative values are ignored, as the XDG
    /// spec requires.
    pub fn resolve() -> Result<StatePaths> {
        Self::resolve_from(std::env::var_os("XDG_STATE_HOME").as_deref(), std::env::var_os("HOME").as_deref())
    }

    /// [`resolve`](Self::resolve) with the variables given (for tests and
    /// for callers that read the environment themselves).
    pub fn resolve_from(
        xdg_state_home: Option<&std::ffi::OsStr>,
        home: Option<&std::ffi::OsStr>,
    ) -> Result<StatePaths> {
        let abs = |v: Option<&std::ffi::OsStr>| v.map(Path::new).filter(|p| p.is_absolute()).map(Path::to_path_buf);
        let base = abs(xdg_state_home)
            .or_else(|| abs(home).map(|h| h.join(".local").join("state")))
            .ok_or(Error::NoStateDir)?;
        Ok(StatePaths::at(base.join(APP_DIR)))
    }

    /// The machine-written blocklist plus the system one, in the order
    /// [`load_blocklists`](crate::load_blocklists) takes them. The user's
    /// hand-written `$XDG_CONFIG_HOME/emoticond/blocklist.txt` is
    /// `emoticond-config`'s to add.
    pub fn blocklist_sources(&self) -> Vec<PathBuf> {
        vec![self.blocklist.clone(), PathBuf::from(crate::SYSTEM_BLOCKLIST)]
    }

    /// Create the directories (writers also create what they need lazily).
    pub fn create_dirs(&self) -> Result<()> {
        for d in [&self.root, &self.sent_dir, &self.overlays, &self.outbox] {
            std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn layout() {
        let p = StatePaths::at("/s/emoticond");
        assert_eq!(p.usage, Path::new("/s/emoticond/usage.json"));
        assert_eq!(p.blocklist, Path::new("/s/emoticond/blocklist.txt"));
        assert_eq!(p.queue, Path::new("/s/emoticond/reports/queue.jsonl"));
        assert_eq!(p.sent_dir, Path::new("/s/emoticond/reports/sent"));
        assert_eq!(p.boosts, Path::new("/s/emoticond/overlays/boosts.jsonl"));
        assert_eq!(p.outbox, Path::new("/s/emoticond/outbox"));
        assert_eq!(p.devlog, Path::new("/s/emoticond/devlog"));
    }

    #[test]
    fn xdg_then_home() {
        let p = StatePaths::resolve_from(Some(OsStr::new("/x/state")), Some(OsStr::new("/home/u"))).unwrap();
        assert_eq!(p.root, Path::new("/x/state/emoticond"));
        let p = StatePaths::resolve_from(Some(OsStr::new("rel")), Some(OsStr::new("/home/u"))).unwrap();
        assert_eq!(p.root, Path::new("/home/u/.local/state/emoticond"));
        let p = StatePaths::resolve_from(None, Some(OsStr::new("/home/u"))).unwrap();
        assert_eq!(p.root, Path::new("/home/u/.local/state/emoticond"));
        assert!(matches!(StatePaths::resolve_from(None, None), Err(Error::NoStateDir)));
    }
}
