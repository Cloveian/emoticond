//! Usage stats (docs/collector.md): sent only when the user said yes
//! (`popularity.mode = shared`, or `emoticond stats on`).
//!
//! Once a day a front-end's daemon sends one [`StatsUpload`]: the day,
//! whether it is this install's first upload of the week and of the month
//! (so the collector can count active installs per day, week and month
//! without any id), the versions, and the finished shared-popularity
//! batches from `outbox/` (bucketed pick counts per shipped search term,
//! [`OutboxBatch`]). Nothing else. A batch is deleted once the collector
//! accepts it; on failure everything stays for the next day's try.

use crate::error::{Error, Result};
use crate::feedback::SendError;
use crate::fsutil::{self, read_optional, write_atomic};
use crate::usage::{OutboxBatch, UsageStore};
use emoticond::PopularityMode;
use serde::{Deserialize, Serialize};

/// The upload format's version.
pub const STATS_VERSION: u16 = 1;

const DAY_MS: u64 = 86_400_000;

/// One day's upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StatsUpload {
    pub v: u16,
    /// Random per upload, for de-duplicating a retry; never reused.
    pub token: String,
    pub engine: String,
    pub data: String,
    /// UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// This install's first upload this week (Monday to Sunday, UTC) and
    /// this month: the collector counts active installs from these.
    pub first_this_week: bool,
    pub first_this_month: bool,
    pub batches: Vec<OutboxBatch>,
}

/// Delivers a [`StatsUpload`]. `Ok` means the collector accepted it.
pub trait StatsSender {
    fn send(&mut self, upload: &StatsUpload) -> std::result::Result<(), SendError>;
}

/// When this install last uploaded (`outbox/ping.json`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
struct PingState {
    /// Days since 1970-01-01 (UTC); 0 is never.
    day: u64,
    week: u64,
    /// `year * 12 + month - 1`.
    month: u64,
}

/// Week number for a day: weeks start on Monday (1970-01-01 was a Thursday).
fn week_of(day: u64) -> u64 {
    (day + 3) / 7
}

/// (year, month 1-12, day 1-31) for days since 1970-01-01 (Howard Hinnant's
/// civil_from_days).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

impl UsageStore {
    fn ping_path(&self) -> std::path::PathBuf {
        self.outbox_dir().join("ping.json")
    }

    /// Send today's upload if the user said yes (mode `Shared`) and none
    /// was sent today. Returns whether one was sent.
    pub fn send_stats(&mut self, sender: &mut dyn StatsSender, now: u64) -> Result<bool> {
        if self.mode() != PopularityMode::Shared {
            return Ok(false);
        }
        let p = self.ping_path();
        let last: PingState = read_optional(&p)?.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let day = now / DAY_MS;
        if day <= last.day {
            return Ok(false);
        }
        self.flush_outbox(now, false)?;
        let batches = self.outbox_batches()?;
        let (y, m, d) = civil(day);
        let month = (y as u64) * 12 + u64::from(m) - 1;
        let stamp = self.stamp();
        let upload = StatsUpload {
            v: STATS_VERSION,
            token: crate::usage::random_token(),
            engine: stamp.engine.clone(),
            data: stamp.data.clone(),
            day: format!("{y:04}-{m:02}-{d:02}"),
            first_this_week: week_of(day) != last.week,
            first_this_month: month != last.month,
            batches: batches.iter().map(|(_, b)| b.clone()).collect(),
        };
        sender.send(&upload).map_err(Error::Send)?;
        for (path, _) in &batches {
            std::fs::remove_file(path).map_err(|e| Error::io(path, e))?;
        }
        let next = PingState { day, week: week_of(day), month };
        write_atomic(&p, fsutil::json_line(&p, &next)?.as_bytes())?;
        Ok(true)
    }
}

/// Posts uploads to `popularity.share_endpoint` (a JSON body, 10 s timeout).
#[cfg(feature = "net")]
#[derive(Debug, Clone)]
pub struct HttpStatsSender {
    pub endpoint: Option<String>,
}

#[cfg(feature = "net")]
impl StatsSender for HttpStatsSender {
    fn send(&mut self, upload: &StatsUpload) -> std::result::Result<(), SendError> {
        let Some(url) = self.endpoint.as_deref().filter(|u| !u.trim().is_empty()) else { return Err(SendError::NoEndpoint) };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .user_agent(concat!("emoticond/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        match agent.post(url).send_json(upload) {
            Ok(_) => Ok(()),
            Err(ureq::Error::StatusCode(code)) => Err(SendError::Rejected(format!("HTTP {code}"))),
            Err(e) => Err(SendError::Transport(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::StatePaths;
    use crate::testutil::TempDir;
    use crate::usage::UsageSettings;
    use emoticond::{FaceId, Pick};

    #[derive(Default)]
    struct Mock {
        got: Vec<StatsUpload>,
        fail: bool,
    }

    impl StatsSender for Mock {
        fn send(&mut self, u: &StatsUpload) -> std::result::Result<(), SendError> {
            if self.fail {
                return Err(SendError::Transport("offline".into()));
            }
            self.got.push(u.clone());
            Ok(())
        }
    }

    fn store(d: &TempDir, mode: PopularityMode) -> UsageStore {
        let s = UsageSettings { mode, refresh_interval_ms: 0, ..Default::default() };
        UsageStore::open_in(&StatePaths::at(d.path()), s)
    }

    #[test]
    fn once_a_day_with_batches_and_period_flags() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Shared);
        s.record_shareable(&Pick::new(FaceId::of_text("a"), Some("shrug".into())), DAY_MS, true).unwrap();
        s.flush_outbox(DAY_MS + 1, true).unwrap();
        let mut m = Mock::default();
        // day 20734 = 2026-10-08, a Thursday
        let t = 20_734 * DAY_MS + 5;
        // offline: nothing changes, it tries again later
        m.fail = true;
        assert!(s.send_stats(&mut m, t).is_err());
        assert_eq!(s.outbox_batches().unwrap().len(), 1);
        m.fail = false;
        assert!(s.send_stats(&mut m, t).unwrap());
        let u = &m.got[0];
        assert_eq!(u.day, "2026-10-08");
        assert!(u.first_this_week && u.first_this_month);
        assert_eq!(u.batches.len(), 1);
        assert!(s.outbox_batches().unwrap().is_empty(), "accepted batches are deleted");
        // once a day
        assert!(!s.send_stats(&mut m, t + 1000).unwrap());
        // the next day: same week and month
        assert!(s.send_stats(&mut m, t + DAY_MS).unwrap());
        let u = &m.got[1];
        assert!(!u.first_this_week && !u.first_this_month && u.batches.is_empty());
        // Monday 2026-10-12: a new week, same month
        assert!(s.send_stats(&mut m, t + 4 * DAY_MS).unwrap());
        assert!(m.got[2].first_this_week && !m.got[2].first_this_month);
    }

    #[test]
    fn nothing_without_shared_mode() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        let mut m = Mock::default();
        assert!(!s.send_stats(&mut m, 20_734 * DAY_MS).unwrap());
        assert!(m.got.is_empty());
    }

    #[test]
    fn calendar() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_734), (2026, 10, 8));
        assert_eq!(civil(11_016), (2000, 2, 29));
        // 2026-10-05 is a Monday: a new week starts there
        assert_eq!(week_of(20_731), week_of(20_737));
        assert_ne!(week_of(20_730), week_of(20_731));
    }
}
