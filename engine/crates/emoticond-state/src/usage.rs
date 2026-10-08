//! Local popularity: `usage.json` (docs/options.md §4).
//!
//! The file holds the core's [`UsageState`]: decayed weights per face and per
//! concept and one "as of" time, never raw query text or per-pick times. All
//! maths is the core's (`emoticond::usage::{record, decay, prune, to_map}`);
//! this type adds the clock, the file, the lock and the reload.
//!
//! Modes ([`PopularityMode`], after the policy ceiling):
//! - `Off`: nothing is recorded and [`UsageStore::map`] is empty. Existing
//!   history stays on disk until [`UsageStore::clear`].
//! - `Local` (the default): picks are recorded here and used here.
//! - `Shared`: as `Local`, plus a per-period tally of picks that rolls into
//!   bucketed, anonymised batches in `outbox/` (options.md §4.3). Nothing in
//!   this crate sends them yet.

use crate::error::{Error, Result};
use crate::fsutil::{self, lock, read_optional, write_atomic};
use crate::paths::StatePaths;
use crate::watch::{Watched, DEFAULT_INTERVAL_MS};
use emoticond::usage as ku;
use emoticond::{EngineStamp, FaceId, Pick, Policy, PopularityMode, UsageMap, UsageState, Warning};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DAY_MS: u64 = 86_400_000;

/// The popularity settings (options.md §4.2) that reach this crate.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UsageSettings {
    /// `popularity.mode`, already capped by the policy (see
    /// [`UsageSettings::apply_policy`]). Default `Local`.
    pub mode: PopularityMode,
    /// `popularity.half_life_days` (1..=3650, default 30).
    pub half_life_days: f32,
    /// `popularity.max_entries`: (term, face) pairs kept, global entries
    /// included (default 5000).
    pub max_entries: usize,
    /// `popularity.remember_terms`: false records and returns only `global`.
    pub remember_terms: bool,
    /// `popularity.share_interval_days`: length of a shared batch period.
    pub share_interval_days: u16,
    /// Engine and data versions written into shared batches.
    pub stamp: EngineStamp,
    /// How often [`UsageStore::refresh`] re-stats the file.
    pub refresh_interval_ms: u64,
}

impl Default for UsageSettings {
    fn default() -> UsageSettings {
        UsageSettings {
            mode: PopularityMode::Local,
            half_life_days: 30.0,
            max_entries: 5000,
            remember_terms: true,
            share_interval_days: 7,
            stamp: EngineStamp::default(),
            refresh_interval_ms: DEFAULT_INTERVAL_MS,
        }
    }
}

impl UsageSettings {
    /// Cap `mode` at the policy's `popularity.max`.
    pub fn apply_policy(&mut self, policy: &Policy) {
        self.mode = policy.popularity(self.mode);
    }
}

#[derive(Debug, Clone, Default)]
struct Loaded {
    state: UsageState,
    warnings: Vec<Warning>,
}

/// Read `usage.json` for display: problems become warnings and an empty
/// state, so a damaged file never stops a front-end from starting.
fn load_lenient(path: &Path) -> Loaded {
    match load_strict(path) {
        Ok((state, None)) => Loaded { state, warnings: Vec::new() },
        Ok((state, Some(w))) => Loaded { state, warnings: vec![w] },
        Err(e) => Loaded {
            state: UsageState::default(),
            warnings: vec![Warning::new("usage_unreadable", None, e.to_string())],
        },
    }
}

/// Read for a read-modify-write: IO errors are errors; a file that does not
/// parse gives an empty state and a warning (the caller backs it up first).
fn load_strict(path: &Path) -> Result<(UsageState, Option<Warning>)> {
    let Some(bytes) = read_optional(path)? else { return Ok((UsageState::default(), None)) };
    match serde_json::from_slice::<UsageState>(&bytes) {
        Ok(s) => Ok((s, None)),
        Err(e) => Ok((
            UsageState::default(),
            Some(Warning::new("usage_unreadable", None, format!("{}: {e}; starting empty", path.display()))),
        )),
    }
}

/// `usage.json` plus its outbox, shared with other processes through the
/// file.
#[derive(Debug)]
pub struct UsageStore {
    path: PathBuf,
    outbox: PathBuf,
    settings: UsageSettings,
    watched: Watched<Loaded>,
}

impl UsageStore {
    /// Open the usage file at `path` (it need not exist) with the outbox
    /// dir for `shared` mode. Never fails: an unreadable file gives an empty
    /// state and a [`warnings`](Self::warnings) entry.
    pub fn open(path: impl Into<PathBuf>, outbox: impl Into<PathBuf>, settings: UsageSettings) -> UsageStore {
        let path = path.into();
        let watched = Watched::file(path.clone(), settings.refresh_interval_ms, load_lenient);
        UsageStore { path, outbox: outbox.into(), settings, watched }
    }

    /// Open `paths.usage` with `paths.outbox`.
    pub fn open_in(paths: &StatePaths, settings: UsageSettings) -> UsageStore {
        UsageStore::open(paths.usage.clone(), paths.outbox.clone(), settings)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn settings(&self) -> &UsageSettings {
        &self.settings
    }

    /// Change settings (a settings page was saved). Takes effect at once.
    pub fn set_settings(&mut self, settings: UsageSettings) {
        self.settings = settings;
    }

    pub fn mode(&self) -> PopularityMode {
        self.settings.mode
    }

    /// The state as last loaded or written (weights as of `state().updated`).
    pub fn state(&self) -> &UsageState {
        &self.watched.get().state
    }

    /// Problems from the last load.
    pub fn warnings(&self) -> &[Warning] {
        &self.watched.get().warnings
    }

    /// Pick up another process's writes: re-stat at most once per
    /// `refresh_interval_ms`, reload if changed. Returns whether it reloaded.
    pub fn refresh(&mut self, now: u64) -> bool {
        self.watched.refresh(now)
    }

    /// The map for `SearchOptions::usage`: the state decayed to `now`.
    /// Empty when the mode is `Off`; `global` only when terms are not
    /// remembered.
    pub fn map(&self, now: u64) -> UsageMap {
        if self.settings.mode == PopularityMode::Off {
            return UsageMap::default();
        }
        let mut st = self.state().clone();
        ku::decay(&mut st, now, self.settings.half_life_days);
        if !self.settings.remember_terms {
            st.by_term.clear();
        }
        ku::to_map(&st)
    }

    /// Record a pick at `now`. Returns false (and writes nothing) when the
    /// mode is `Off`. In `Shared` mode the pick's concept is **not** shared;
    /// use [`record_shareable`](Self::record_shareable) for picks whose
    /// `term_key` is a shipped vocab or phrase key.
    pub fn record(&mut self, pick: &Pick, now: u64) -> Result<bool> {
        self.record_shareable(pick, now, false)
    }

    /// Record a pick; `term_is_shipped` says the pick's `term_key` comes from
    /// shipped vocab or phrase keys (never a personal overlay phrase or free
    /// text), so `Shared` mode may count it under that term. Otherwise
    /// shared batches count the face without a term.
    pub fn record_shareable(&mut self, pick: &Pick, now: u64, term_is_shipped: bool) -> Result<bool> {
        if self.settings.mode == PopularityMode::Off {
            return Ok(false);
        }
        let mut pick = pick.clone();
        if !self.settings.remember_terms {
            pick.term_key = None;
        }
        let hl = self.settings.half_life_days;
        self.update(now, |st| {
            ku::decay(st, now, hl);
            ku::record(st, &pick, now);
        })?;
        if self.settings.mode == PopularityMode::Shared {
            let term = pick.term_key.clone().filter(|_| term_is_shipped);
            self.tally(term, pick.id, now)?;
        }
        Ok(true)
    }

    /// Read-modify-write the state under the writer lock, then prune to
    /// `max_entries` and replace the file atomically. Ignores the mode: it is
    /// for explicit actions such as an import. A file that no longer parses
    /// is kept as `usage.json.bad-<now>` before being overwritten.
    pub fn update(&mut self, now: u64, f: impl FnOnce(&mut UsageState)) -> Result<()> {
        let _g = lock(&self.path)?;
        let (mut st, bad) = load_strict(&self.path)?;
        if bad.is_some() {
            let backup = fsutil::sibling(&self.path, &format!(".bad-{now}"));
            std::fs::copy(&self.path, &backup).map_err(|e| Error::io(&backup, e))?;
        }
        f(&mut st);
        ku::prune(&mut st, self.settings.max_entries);
        let bytes = serde_json::to_vec(&st).map_err(|e| Error::Json { path: self.path.clone(), source: e })?;
        write_atomic(&self.path, &bytes)?;
        self.watched.set_written(Loaded { state: st, warnings: Vec::new() });
        Ok(())
    }

    /// "Clear history": forget every pick, and drop the shared tally and any
    /// unsent shared batches. Works in every mode.
    pub fn clear(&mut self) -> Result<()> {
        let hl = self.settings.half_life_days;
        self.update(0, |st| {
            st.clear();
            st.half_life_days = hl;
        })?;
        let _g = lock(&self.path)?;
        let _ = std::fs::remove_file(self.tally_path());
        for (p, _) in self.outbox_batches()? {
            std::fs::remove_file(&p).map_err(|e| Error::io(&p, e))?;
        }
        Ok(())
    }

    // ---- shared mode -----------------------------------------------------

    fn tally_path(&self) -> PathBuf {
        self.outbox.join("pending.json")
    }

    fn interval_ms(&self) -> u64 {
        u64::from(self.settings.share_interval_days.max(1)) * DAY_MS
    }

    fn tally(&mut self, term: Option<String>, id: FaceId, now: u64) -> Result<()> {
        let _g = lock(&self.path)?;
        let mut t = self.read_tally()?;
        self.roll_over(&mut t, now, false)?;
        if t.from == 0 {
            t.from = now;
        }
        *t.counts.entry(term.unwrap_or_default()).or_default().entry(id).or_insert(0) += 1;
        self.write_tally(&t)
    }

    fn read_tally(&self) -> Result<Tally> {
        let p = self.tally_path();
        Ok(read_optional(&p)?.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default())
    }

    fn write_tally(&self, t: &Tally) -> Result<()> {
        let p = self.tally_path();
        write_atomic(&p, fsutil::json_line(&p, t)?.as_bytes())
    }

    /// If the period is over (or `force`), write the tally as a batch and
    /// start a new period at `now`. The caller holds the lock.
    fn roll_over(&self, t: &mut Tally, now: u64, force: bool) -> Result<Option<PathBuf>> {
        if t.from == 0 || !(force || now.saturating_sub(t.from) >= self.interval_ms()) {
            return Ok(None);
        }
        let mut out = None;
        if !t.counts.is_empty() {
            let batch = OutboxBatch {
                v: 1,
                token: random_token(),
                from: t.from,
                to: now,
                engine: self.settings.stamp.engine.clone(),
                data: self.settings.stamp.data.clone(),
                counts: t
                    .counts
                    .iter()
                    .flat_map(|(term, m)| {
                        m.iter().map(move |(id, n)| OutboxCount {
                            term: Some(term.clone()).filter(|t| !t.is_empty()),
                            id: *id,
                            bucket: Bucket::of(*n),
                        })
                    })
                    .collect(),
            };
            let p = self.outbox.join(format!("usage-{}-{}.json", t.from, now));
            write_atomic(&p, fsutil::json_line(&p, &batch)?.as_bytes())?;
            out = Some(p);
        }
        *t = Tally { v: 1, from: now, counts: BTreeMap::new() };
        Ok(out)
    }

    /// Close the current shared period if it is due (or `force`), writing
    /// its batch to the outbox. Returns the batch file, if one was written.
    /// Does nothing unless the mode is `Shared`.
    pub fn flush_outbox(&mut self, now: u64, force: bool) -> Result<Option<PathBuf>> {
        if self.settings.mode != PopularityMode::Shared {
            return Ok(None);
        }
        let _g = lock(&self.path)?;
        let mut t = self.read_tally()?;
        let out = self.roll_over(&mut t, now, force)?;
        if out.is_some() || t.from == now {
            self.write_tally(&t)?;
        }
        Ok(out)
    }

    /// The batches waiting in the outbox, oldest first: the exact payloads
    /// for a "what will be sent" view. Unreadable files are skipped.
    pub fn outbox_batches(&self) -> Result<Vec<(PathBuf, OutboxBatch)>> {
        let rd = match std::fs::read_dir(&self.outbox) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(&self.outbox, e)),
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("usage-") && name.ends_with(".json")) {
                continue;
            }
            if let Some(b) = read_optional(&p)?.and_then(|b| serde_json::from_slice::<OutboxBatch>(&b).ok()) {
                out.push((p, b));
            }
        }
        out.sort_by(|a, b| (a.1.from, &a.0).cmp(&(b.1.from, &b.0)));
        Ok(out)
    }
}

/// The running tally of the current shared period (`outbox/pending.json`).
/// Raw counts, but only ever on this machine; batches carry buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Tally {
    v: u16,
    from: u64,
    /// term ("" for none) -> face -> picks this period
    counts: BTreeMap<String, BTreeMap<FaceId, u32>>,
}

/// A pick count rounded into a bucket (options.md §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Bucket {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "2-4")]
    Few,
    #[serde(rename = "5-19")]
    Some,
    #[serde(rename = "20+")]
    Many,
}

impl Bucket {
    pub fn of(n: u32) -> Bucket {
        match n {
            0 | 1 => Bucket::One,
            2..=4 => Bucket::Few,
            5..=19 => Bucket::Some,
            _ => Bucket::Many,
        }
    }
}

/// One shared-usage batch (options.md §4.3): per-concept bucketed counts,
/// the period, versions and a fresh random token for de-duplicating
/// retries. No install id, user id, locale or timezone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OutboxBatch {
    pub v: u16,
    pub token: String,
    /// Period start and end, unix ms.
    pub from: u64,
    pub to: u64,
    pub engine: String,
    pub data: String,
    pub counts: Vec<OutboxCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OutboxCount {
    /// A shipped vocab or phrase key, or none.
    pub term: Option<String>,
    pub id: FaceId,
    pub bucket: Bucket,
}

/// 32 hex digits from std's randomly keyed hasher (no RNG dependency).
/// Only used to de-duplicate retries, so it need not be cryptographic.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut s = String::with_capacity(32);
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(nanos);
        h.write_u32(std::process::id());
        h.write_u64(N.fetch_add(1, Ordering::Relaxed));
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const DAY: u64 = DAY_MS;

    fn id(t: &str) -> FaceId {
        FaceId::of_text(t)
    }

    fn store(d: &TempDir, mode: PopularityMode) -> UsageStore {
        let s = UsageSettings { mode, refresh_interval_ms: 0, ..Default::default() };
        UsageStore::open_in(&StatePaths::at(d.path()), s)
    }

    #[test]
    fn default_mode_is_local() {
        assert_eq!(UsageSettings::default().mode, PopularityMode::Local);
    }

    #[test]
    fn record_persists_and_reloads() {
        let d = TempDir::new();
        let mut a = store(&d, PopularityMode::Local);
        assert!(a.record(&Pick::new(id("x"), Some("idk".into())), 1000).unwrap());
        let b = store(&d, PopularityMode::Local);
        assert_eq!(b.state().global[&id("x")], 1.0);
        assert_eq!(b.map(1000).weight(id("x"), Some("idk")), 0.5);
    }

    #[test]
    fn off_records_nothing_and_maps_nothing() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        s.record(&Pick::new(id("x"), None), 5).unwrap();
        let mut off = store(&d, PopularityMode::Off);
        assert!(!off.record(&Pick::new(id("y"), None), 6).unwrap());
        assert!(off.map(6).is_empty());
        // history is kept until clear
        assert_eq!(off.state().global.len(), 1);
        off.clear().unwrap();
        assert!(store(&d, PopularityMode::Local).state().is_empty());
    }

    #[test]
    fn decay_over_injected_time() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        s.record(&Pick::new(id("x"), Some("t".into())), 0).unwrap();
        // one half-life later the raw weight is 0.5, squashed 0.5/1.5
        let m = s.map(30 * DAY);
        assert!((m.global[&id("x")] - 0.5 / 1.5).abs() < 1e-5);
        // a second pick then decays the first before adding
        s.record(&Pick::new(id("x"), None), 30 * DAY).unwrap();
        assert!((s.state().global[&id("x")] - 1.5).abs() < 1e-5);
        // and after a very long time it is forgotten
        assert!(s.map(3000 * DAY).is_empty());
    }

    #[test]
    fn prune_to_max_entries() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        let mut set = s.settings().clone();
        set.max_entries = 3;
        s.set_settings(set);
        for t in ["a", "b", "c", "d"] {
            s.record(&Pick::new(id(t), None), 1).unwrap();
        }
        s.record(&Pick::new(id("d"), None), 1).unwrap();
        assert_eq!(s.state().len(), 3);
        assert!(s.state().global.contains_key(&id("d")));
    }

    #[test]
    fn remember_terms_off_keeps_global_only() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        let mut set = s.settings().clone();
        set.remember_terms = false;
        s.set_settings(set);
        s.record(&Pick::new(id("a"), Some("secret".into())), 1).unwrap();
        assert!(s.state().by_term.is_empty());
        assert!(!std::fs::read_to_string(s.path()).unwrap().contains("secret"));
    }

    #[test]
    fn corrupt_file_warns_and_is_backed_up_on_write() {
        let d = TempDir::new();
        let p = StatePaths::at(d.path());
        std::fs::write(&p.usage, b"{not json").unwrap();
        let mut s = store(&d, PopularityMode::Local);
        assert_eq!(s.warnings().len(), 1);
        assert!(s.state().is_empty());
        s.record(&Pick::new(id("a"), None), 77).unwrap();
        assert_eq!(std::fs::read(fsutil::sibling(&p.usage, ".bad-77")).unwrap(), b"{not json");
    }

    #[test]
    fn policy_caps_mode() {
        let mut s = UsageSettings { mode: PopularityMode::Shared, ..Default::default() };
        let mut p = Policy::default();
        p.max_popularity = Some(PopularityMode::Local);
        s.apply_policy(&p);
        assert_eq!(s.mode, PopularityMode::Local);
    }

    #[test]
    fn shared_mode_writes_bucketed_batches() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Shared);
        for _ in 0..3 {
            s.record_shareable(&Pick::new(id("a"), Some("shrug".into())), DAY, true).unwrap();
        }
        // a personal term is never shared
        s.record(&Pick::new(id("b"), Some("my private phrase".into())), DAY).unwrap();
        assert!(s.outbox_batches().unwrap().is_empty());
        // the next pick after the period closes the batch
        s.record_shareable(&Pick::new(id("a"), Some("shrug".into())), 9 * DAY, true).unwrap();
        let batches = s.outbox_batches().unwrap();
        assert_eq!(batches.len(), 1);
        let b = &batches[0].1;
        assert_eq!((b.from, b.to), (DAY, 9 * DAY));
        assert_eq!(b.token.len(), 32);
        assert_eq!(b.counts.len(), 2);
        assert!(b.counts.contains(&OutboxCount { term: Some("shrug".into()), id: id("a"), bucket: Bucket::Few }));
        assert!(b.counts.contains(&OutboxCount { term: None, id: id("b"), bucket: Bucket::One }));
        let raw = std::fs::read_to_string(&batches[0].0).unwrap();
        assert!(!raw.contains("private"));
        assert!(raw.contains("\"2-4\""));
        // forced flush closes the current period
        assert!(s.flush_outbox(9 * DAY + 1, true).unwrap().is_some());
        assert_eq!(s.outbox_batches().unwrap().len(), 2);
        s.clear().unwrap();
        assert!(s.outbox_batches().unwrap().is_empty());
    }

    #[test]
    fn local_mode_writes_no_outbox() {
        let d = TempDir::new();
        let mut s = store(&d, PopularityMode::Local);
        s.record_shareable(&Pick::new(id("a"), Some("shrug".into())), 1, true).unwrap();
        assert!(!d.path().join("outbox").exists());
    }

    #[test]
    fn buckets() {
        assert_eq!(
            [1, 2, 4, 5, 19, 20, 999].map(Bucket::of),
            [Bucket::One, Bucket::Few, Bucket::Few, Bucket::Some, Bucket::Some, Bucket::Many, Bucket::Many]
        );
    }
}
