//! Everything a front-end re-checks before a search
//!.
//!
//! ```text
//! let changes = shared.refresh(now_ms());
//! if changes.blocklist { db.set_blocklist(shared.blocklist().ids.clone()) }
//! if changes.overlays  { db.reload_overlays(..) }
//! opts.usage = shared.usage_map(now_ms());
//! ```
//!
//! That is the whole protocol between processes: an offensive report made
//! in one launcher is hidden in the picker on its next keystroke.

use crate::blocklist::{load_blocklists, owned, Blocklist};
use crate::paths::StatePaths;
use crate::usage::{UsageSettings, UsageStore};
use crate::watch::Watched;
use emoticond::UsageMap;
use std::path::{Path, PathBuf};

/// What [`Shared::refresh`] found changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Changes {
    pub usage: bool,
    pub blocklist: bool,
    pub overlays: bool,
}

impl Changes {
    pub fn any(self) -> bool {
        self.usage || self.blocklist || self.overlays
    }
}

/// The watched usage, blocklist and overlay files.
#[derive(Debug)]
pub struct Shared {
    pub usage: UsageStore,
    blocklist: Watched<Blocklist>,
    overlays: Watched<()>,
}

impl Shared {
    /// Watch `paths.usage`, the blocklist files `blocklists` (typically
    /// [`StatePaths::blocklist_sources`] plus the user's config-dir file)
    /// and the overlay files `overlays` (the state `boosts.jsonl` plus the
    /// config dir's overlays). The interval comes from
    /// `settings.refresh_interval_ms`.
    pub fn open(
        paths: &StatePaths,
        settings: UsageSettings,
        blocklists: &[impl AsRef<Path>],
        overlays: &[impl AsRef<Path>],
    ) -> Shared {
        let interval = settings.refresh_interval_ms;
        Shared {
            usage: UsageStore::open_in(paths, settings),
            blocklist: Watched::new(owned(blocklists), interval, |ps: &[PathBuf]| load_blocklists(ps)),
            overlays: Watched::new(owned(overlays), interval, |_: &[PathBuf]| ()),
        }
    }

    /// Re-stat (at most once per interval) and reload what changed.
    pub fn refresh(&mut self, now: u64) -> Changes {
        Changes {
            usage: self.usage.refresh(now),
            blocklist: self.blocklist.refresh(now),
            overlays: self.overlays.refresh(now),
        }
    }

    /// The merged blocklists as last loaded.
    pub fn blocklist(&self) -> &Blocklist {
        self.blocklist.get()
    }

    /// The overlay files to hand to `OpenOptions::overlays` / `reload_overlays`.
    pub fn overlay_paths(&self) -> &[PathBuf] {
        self.overlays.paths()
    }

    /// `SearchOptions::usage` for a query at `now`.
    pub fn usage_map(&self, now: u64) -> UsageMap {
        self.usage.map(now)
    }

    /// Reload the blocklist now (after this process changed it).
    pub fn reload_blocklist(&mut self) {
        self.blocklist.reload();
    }
}
