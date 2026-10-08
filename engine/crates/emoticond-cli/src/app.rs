//! Config and state, wired once for every command and the daemon.
//!
//! - emoticond-config's `Loader` resolves the settings for a front-end
//!   (`[profile.<name>]`: `cli`, `menu`, `serve`, `quickshell`, ...), with
//!   command-line overrides on top and the policy over everything.
//! - emoticond-state keeps what this machine learns, under
//!   `$XDG_STATE_HOME/emoticond/`: `usage.json` (local popularity, on by
//!   default), `blocklist.txt` (offensive reports), `reports/queue.jsonl`
//!   (every report, queued: there is no endpoint yet) and
//!   `overlays/boosts.jsonl` ("doesn't fit").
//! - `Shared` re-reads the usage and blocklist files when another process
//!   changes them (at most one stat per second), so a face reported as
//!   offensive in one front-end is gone from the others on their next search.
//! - The dev pick log (`dev.pick_log`, `EMOTICOND_PICK_LOG`): every pick
//!   with what was shown, for building eval sets. Off unless set.

use emoticond::{Database, FaceId, LocalEffect, OverlaySource, Pick, PopularityMode, Reason, Report, SearchOptions, UsageMap, Warning};
use emoticond_config::{ConfigError, Loader, ReportsChoice, Resolved};
use emoticond_state::{
    disclaimer_footer, remove_from_blocklist, undemote, DevLog, FeedbackQueue, LocalEffects, PickLogRecord, QueueLimits, SendOff,
    SendPolicy, Shared, StatePaths, UsageSettings,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub use emoticond_state::now_ms;

/// The config choices a command line can make (options.md §10.3).
#[derive(Debug, Clone, Default)]
pub struct ConfigArgs {
    /// `--config PATH`
    pub config: Option<PathBuf>,
    /// `--no-config`
    pub no_config: bool,
    /// `--profile NAME`
    pub profile: Option<String>,
    /// `--set key=value` and the per-option flags (`--safety moderate`).
    pub sets: Vec<String>,
}

/// Resolve the settings for front-end `frontend`.
pub fn load_config(frontend: &str, a: &ConfigArgs) -> Result<Resolved, ConfigError> {
    let mut l = Loader::new().frontend(frontend);
    if let Some(p) = &a.config {
        l = l.config_file(p);
    }
    if a.no_config {
        l = l.no_config();
    }
    if let Some(p) = &a.profile {
        l = l.profile(p);
    }
    l.set_all(a.sets.iter().cloned()).load()
}

/// What a report did, for the response and the CLI.
#[derive(Debug, Clone, Default)]
pub struct Submitted {
    pub report_id: String,
    pub key: String,
    /// `blocked`, `unblocked`, `picked`, `demoted`, `undemoted`.
    pub effects: Vec<&'static str>,
    /// Effects not applied (`feedback.apply_locally = false`, popularity off).
    pub skipped: Vec<&'static str>,
    /// Faces to leave out of results from now on.
    pub blocked: Vec<FaceId>,
    pub unblocked: Vec<FaceId>,
    /// A local effect failed (the report is still queued).
    pub error: Option<String>,
}

/// Whether reports are sent: the user's choice (`feedback.send`, else their
/// answer to "send reports?", else not asked: off) and the packager's
/// policy. Even when on, nothing leaves the machine until an endpoint
/// exists; reports wait in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sending {
    pub on: bool,
    /// `"user"`, `"policy"` or `"unasked"` when off.
    pub off_by: Option<&'static str>,
}

impl Sending {
    /// The one-line footer under a report menu.
    pub fn footer(self) -> &'static str {
        disclaimer_footer(match self.off_by {
            None => SendPolicy::Allowed,
            Some("policy") => SendPolicy::Off(SendOff::Policy),
            Some("unasked") => SendPolicy::Off(SendOff::Unasked),
            Some(_) => SendPolicy::Off(SendOff::User),
        })
    }
}

/// Settings plus state for one process.
pub struct App {
    pub cfg: Resolved,
    pub paths: StatePaths,
    pub shared: Shared,
    pub queue: FeedbackQueue,
    pub pick_log: DevLog,
    /// Blocked by this process: hidden at once, even if writing the
    /// blocklist file failed.
    blocked_here: BTreeSet<FaceId>,
    /// Overlay layers, lowest first: this machine's state overlays (where
    /// "doesn't fit" demotes land), then the config's (user dir, data.overlays).
    overlay_sources: Vec<OverlaySource>,
    /// A report here wrote an overlay: reload before the next query.
    overlays_dirty: bool,
}

impl App {
    pub fn new(cfg: Resolved) -> App {
        let c = &cfg.config;
        let mut paths = StatePaths::at(cfg.paths.state_dir.clone());
        if let Some(p) = &c.popularity.store {
            paths.usage = p.clone();
        }
        if let Some(p) = &c.feedback.queue {
            paths.queue = p.clone();
        }
        let mut us = UsageSettings::default();
        us.mode = cfg.popularity_mode();
        us.half_life_days = c.popularity.half_life_days;
        us.max_entries = c.popularity.max_entries as usize;
        us.remember_terms = c.popularity.remember_terms;
        us.share_interval_days = c.popularity.share_interval_days;
        us.stamp.engine = emoticond::VERSION.to_string();
        let mut blocklists = cfg.blocklist_files.clone();
        blocklists.push(cfg.paths.user_blocklist());
        if !blocklists.contains(&paths.blocklist) {
            blocklists.insert(0, paths.blocklist.clone());
        }
        let mut overlay_sources = vec![OverlaySource::Path(paths.overlays.clone())];
        for d in &cfg.overlays {
            if *d != paths.overlays {
                overlay_sources.push(OverlaySource::Path(d.clone()));
            }
        }
        // watch the overlay files themselves: editing a file in place does
        // not change its directory's mtime
        let watch: Vec<PathBuf> = overlay_sources.iter().flat_map(|s| s.watch_paths()).collect();
        let shared = Shared::open(&paths, us, &blocklists, &watch);
        let mut limits = QueueLimits::default();
        limits.queue_max = c.feedback.queue_max as usize;
        limits.max_age_days = c.feedback.queue_max_age_days;
        let queue = FeedbackQueue::at(paths.queue.clone(), paths.sent_log.clone(), limits);
        let pick_log = DevLog::from_setting(c.dev.pick_log.as_deref().and_then(|p| p.to_str()));
        App { cfg, paths, shared, queue, pick_log, blocked_here: BTreeSet::new(), overlay_sources, overlays_dirty: false }
    }

    /// Config and state warnings worth showing once (at start).
    pub fn warnings(&self) -> Vec<Warning> {
        let mut w = self.cfg.warnings.clone();
        w.extend(self.shared.blocklist().warnings.iter().cloned());
        w.extend(self.shared.usage.warnings().iter().cloned());
        w
    }

    /// Pick up other processes' writes (usage, blocklist, overlays) and
    /// this process's own overlay writes, before a query.
    pub fn refresh(&mut self, db: Option<&Database>) {
        let ch = self.shared.refresh(now_ms());
        if ch.overlays || self.overlays_dirty {
            self.load_overlays(db);
        }
    }

    /// Apply the overlay layers to the database now (at open, and when they
    /// change). Returns the overlays' warnings (bad lines and the like).
    pub fn load_overlays(&mut self, db: Option<&Database>) -> Vec<Warning> {
        self.overlays_dirty = false;
        match db {
            Some(db) => db.reload_overlays(&self.overlay_sources),
            None => Vec::new(),
        }
    }

    pub fn popularity(&self) -> PopularityMode {
        self.shared.usage.mode()
    }

    /// Every face that must not be shown: the blocklist files (state, user,
    /// system, policy) and what this process blocked.
    pub fn blocked(&self) -> BTreeSet<FaceId> {
        let mut b = self.shared.blocklist().ids.clone();
        b.extend(self.blocked_here.iter().copied());
        b
    }

    /// Local popularity for a query now (empty when popularity is off).
    pub fn usage_map(&self) -> UsageMap {
        self.shared.usage_map(now_ms())
    }

    /// The configured per-query options with a request's `opts` layered in
    /// (options.md §7.2: between the profile and env, clamped by policy).
    pub fn options_with(&self, opts: &serde_json::Map<String, serde_json::Value>) -> Result<(SearchOptions, Vec<Warning>), ConfigError> {
        if opts.is_empty() {
            return Ok((self.cfg.search.clone(), Vec::new()));
        }
        let r = self.cfg.with_request(&serde_json::Value::Object(opts.clone()))?;
        Ok((r.options, r.warnings))
    }

    /// The last step for every query: usage and the blocklist as
    /// `exclude`. Usage is the request's own (`keep_usage`: it sent
    /// `opts.usage`), else the client's pushed map, else the usage store.
    pub fn finish(&self, o: &mut SearchOptions, keep_usage: bool, client: Option<&UsageMap>) {
        if !keep_usage {
            o.usage = match client {
                Some(u) => u.clone(),
                None => self.usage_map(),
            };
        }
        self.cfg.policy.clamp(o);
        o.exclude.extend(self.blocked());
    }

    pub fn sending(&self) -> Sending {
        if self.cfg.policy.reports_disabled {
            return Sending { on: false, off_by: Some("policy") };
        }
        match self.cfg.reports_choice() {
            ReportsChoice::Unasked => Sending { on: false, off_by: Some("unasked") },
            c if !c.send() => Sending { on: false, off_by: Some("user") },
            _ => Sending { on: true, off_by: None },
        }
    }

    /// Save the answer to "send reports?" (`consent.json`). Refused when
    /// `feedback.send` is set in a config file, the environment or a flag
    /// (that wins anyway) or the policy has turned sending off.
    pub fn set_consent(&self, send: bool) -> Result<(), String> {
        if self.cfg.policy.reports_disabled {
            return Err("sending is turned off by your system administrator".into());
        }
        if let ReportsChoice::Set(_) = self.cfg.reports_choice() {
            let from = self.cfg.source("feedback.send").map(|s| s.to_string()).unwrap_or_default();
            return Err(format!("feedback.send is set in {from}, which wins; change it there"));
        }
        let c = emoticond_config::Consent { send, since_ms: now_ms() };
        emoticond_config::consent::write(&self.cfg.paths.consent_file(), &c).map_err(|e| format!("saving {}: {e}", self.cfg.paths.consent_file().display()))
    }

    /// What a background sender needs: the queue and the endpoint. None
    /// when there is nowhere to send. Whether to send is checked on every
    /// round ([`send_now`]), so a later answer takes effect.
    #[cfg(feature = "net")]
    pub fn sender_parts(&self) -> Option<(FeedbackQueue, String, Resolved)> {
        let endpoint = self.cfg.config.feedback.endpoint.clone().filter(|e| !e.trim().is_empty())?;
        Some((self.queue.clone(), endpoint, self.cfg.clone()))
    }

    /// Record a pick: local popularity (unless off) and the dev pick log
    /// (when on). Returns (recorded in usage, written to the dev log).
    pub fn pick(&mut self, pick: &Pick, shipped_term: bool, log: Option<&PickLogRecord>) -> (bool, bool, Option<String>) {
        let now = now_ms();
        let mut err = None;
        let recorded = match self.shared.usage.record_shareable(pick, now, shipped_term) {
            Ok(r) => r,
            Err(e) => {
                err = Some(e.to_string());
                false
            }
        };
        let logged = match log {
            Some(rec) => self.pick_log.log_pick(rec, now).unwrap_or_else(|e| {
                err.get_or_insert(e.to_string());
                false
            }),
            None => false,
        };
        (recorded, logged, err)
    }

    /// Add a face to the state blocklist and hide it here at once.
    pub fn block(&mut self, id: FaceId, text: Option<&str>) -> emoticond_state::Result<bool> {
        self.blocked_here.insert(id);
        let r = emoticond_state::add_to_blocklist_noted(&self.paths.blocklist, id, text);
        self.shared.reload_blocklist();
        r
    }

    /// Remove a face from the state blocklist (the "Undo" after a report).
    pub fn unblock(&mut self, id: FaceId) -> emoticond_state::Result<bool> {
        self.blocked_here.remove(&id);
        let r = remove_from_blocklist(&self.paths.blocklist, id);
        self.shared.reload_blocklist();
        r
    }

    /// For a `clear` that didn't say what it clears: the newest pending
    /// choice for the same query and target (the menu item being un-chosen).
    pub fn last_choice(&self, query: &str, target: &emoticond::Target) -> Option<Reason> {
        let q = query.trim().to_lowercase();
        let same = |t: &emoticond::Target| match (t, target) {
            (emoticond::Target::Query, emoticond::Target::Query) => true,
            (emoticond::Target::Face { id: a, .. }, emoticond::Target::Face { id: b, .. }) => a == b,
            _ => false,
        };
        let pending = self.queue.pending(now_ms()).ok()?;
        pending
            .reports
            .into_iter()
            .rev()
            .find(|r| r.reason != Reason::Clear && same(&r.target) && r.query.trim().to_lowercase() == q)
            .map(|r| r.reason)
    }

    /// Queue a report and apply its local effects (options.md §5.1):
    /// offensive blocks the face (always), great fit is a pick, doesn't fit
    /// demotes it for the concept, and clear undoes the earlier report with
    /// the same key (unblock / undemote).
    pub fn submit(&mut self, report: &Report, effects: &[LocalEffect]) -> emoticond_state::Result<Submitted> {
        let now = now_ms();
        let mut out = Submitted { report_id: report.report_id.clone(), key: report.key.clone(), ..Submitted::default() };
        if report.reason == Reason::Clear {
            let prev = self.queue.pending(now).ok().and_then(|p| p.reports.into_iter().rev().find(|r| r.key == report.key));
            if let (Some(prev), emoticond::Target::Face { id, .. }) = (prev, &report.target) {
                match prev.reason {
                    Reason::Offensive => match self.unblock(*id) {
                        Ok(_) => {
                            out.effects.push("unblocked");
                            out.unblocked.push(*id);
                        }
                        Err(e) => out.error = Some(e.to_string()),
                    },
                    Reason::NoFit => {
                        if let Some(k) = &prev.term_key {
                            match undemote(&self.paths.boosts, k, *id) {
                                Ok(true) => {
                                    out.effects.push("undemoted");
                                    self.overlays_dirty = true;
                                }
                                Ok(false) => {}
                                Err(e) => out.error = Some(e.to_string()),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let apply_locally = self.cfg.config.feedback.apply_locally;
        // the offensive hide is immediate even if a file write fails
        for e in effects {
            if let LocalEffect::Block { id, .. } = e {
                self.blocked_here.insert(*id);
            }
        }
        // FeedbackQueue::submit, in two steps: a failed local effect is a
        // warning, a report that could not be queued an error
        let applied = LocalEffects::new(&self.paths, Some(&mut self.shared.usage), apply_locally).apply(effects, now);
        self.shared.reload_blocklist();
        self.queue.append(report, now)?;
        match applied {
            Ok(a) => {
                if !a.blocked.is_empty() {
                    out.effects.push("blocked");
                }
                if !a.picked.is_empty() {
                    out.effects.push("picked");
                }
                if !a.demoted.is_empty() {
                    out.effects.push("demoted");
                    self.overlays_dirty = true;
                }
                for s in &a.skipped {
                    out.skipped.push(match s {
                        LocalEffect::Block { .. } => "block",
                        LocalEffect::RecordPick { .. } => "pick",
                        LocalEffect::Demote { .. } => "demote",
                        _ => "other",
                    });
                }
                out.blocked = a.blocked;
            }
            Err(e) => out.error = Some(e.to_string()),
        }
        Ok(out)
    }
}

/// A random report id (16 hex digits). The library has no RNG; the kernel
/// does, with a time-and-pid fallback.
pub fn random_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !ok {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(now_ms());
        h.write_u32(std::process::id());
        b = h.finish().to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The send policy right now, and the oldest report it may send.
#[cfg(feature = "net")]
pub fn send_now(cfg: &Resolved) -> (SendPolicy, u64) {
    let choice = cfg.reports_choice();
    let policy = match choice {
        ReportsChoice::Unasked if !cfg.policy.reports_disabled => SendPolicy::Off(emoticond_state::SendOff::Unasked),
        c => SendPolicy::new(&cfg.policy, c.send()),
    };
    (policy, choice.since_ms())
}
