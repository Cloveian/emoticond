//! emoticond-state: the files a kaomoji front-end writes
//! (docs/options.md §4, §5, §7.1).
//!
//! The core `emoticond` crate is pure: no clock, no IO. This crate keeps the
//! state a front-end builds up on one machine, and supplies the clock:
//!
//! - [`StatePaths`]: where everything lives (`$XDG_STATE_HOME/emoticond/`).
//! - [`UsageStore`]: local popularity in `usage.json` (on by default), and
//!   the anonymised outbox for `shared` mode.
//! - [`FeedbackQueue`]: the report queue, supersession by key, and sending
//!   through a [`Sender`] that both the user and the packager can switch off.
//! - [`apply_local_effects`]: what a report does on this machine at once
//!   (blocklist, pick, demote).
//! - [`load_blocklists`] / [`add_to_blocklist`]: the blocklist files.
//! - [`DevLog`]: an opt-in pick log, for development.
//! - [`import_picks`]: one-off import of `work/eval/picks.jsonl`.
//! - [`Watched`] and [`Shared`]: reload-on-change, so several processes
//!   (a launcher plugin, the picker, the CLI) share the files without a
//!   daemon.
//!
//! Every whole-file write goes to a temp file, is synced and renamed over the
//! target, under an advisory lock on a sibling `.lock` file. Append-only
//! files append one line per write under the same kind of lock. Readers
//! never lock and never see a half-written file.
//!
//! Times are unix milliseconds, as in the core. Functions that need the time
//! take `now` so tests can inject it; [`now_ms`] reads the system clock.

pub mod blocklist;
pub mod devlog;
pub mod effects;
pub mod error;
pub mod feedback;
mod fsutil;
pub mod import;
pub mod overlay;
pub mod paths;
pub mod shared;
pub mod stats;
pub mod usage;
pub mod watch;

pub use blocklist::{
    add_to_blocklist, add_to_blocklist_noted, load_blocklists, parse_blocklist, remove_from_blocklist, Blocklist, SYSTEM_BLOCKLIST,
};
pub use devlog::{DevLog, PickLogRecord, RatingLogRecord};
pub use effects::{apply_local_effects, Applied, LocalEffects};
pub use error::{Error, Result};
#[cfg(feature = "net")]
pub use feedback::HttpSender;
pub use feedback::{
    disclaimer_footer, FeedbackQueue, FlushOutcome, Pending, QueueLimits, SendError, SendOff, SendPolicy, Sender,
    DISCLAIMER, DISCLAIMER_VERSION,
};
pub use import::{import_picks, normalize_query, ImportSummary};
pub use overlay::{demote, read_boosts, undemote, BoostRow, DEMOTE_BOOST};
pub use paths::StatePaths;
pub use shared::{Changes, Shared};
#[cfg(feature = "net")]
pub use stats::HttpStatsSender;
pub use stats::{StatsSender, StatsUpload, STATS_VERSION};
pub use usage::{Bucket, OutboxBatch, OutboxCount, UsageSettings, UsageStore};
pub use watch::Watched;

/// The system clock as unix milliseconds (0 if the clock is before 1970).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A temp dir removed on drop (no `tempfile` dependency needed).
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "emoticond-state-unit-{}-{}-{}",
                std::process::id(),
                crate::now_ms(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
