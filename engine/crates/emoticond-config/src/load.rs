//! Loading and precedence (docs/options.md §7.2):
//!
//! ```text
//! policy locks > CLI > env > daemon request > profile > user config > system config > defaults
//! policy ceilings clamp the final result
//! ```
//!
//! Profiles: `[profile.<name>]` tables override the top level of the
//! **same** file. A system profile sits between the system top level and
//! the user config, so a user's own file always beats the system's:
//! `system < system profile < user < user profile`.

use crate::codes;
use crate::config::Config;
use crate::env::Env;
use crate::paths::{dedup, Paths};
use crate::policy::{Locks, PolicyFile};
use crate::schema::{self, canonical_key, Page, SPECS};
use crate::value::{Bad, Ctx, Value};
use crate::walk::{self, Entry, Walker};
use crate::ConfigError;
use emoticond::{Explain, FaceId, OpenOptions, OverlaySource, Policy, PopularityMode, SearchOptions, UsageMap, UsageWeight, Warning};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// Where an effective value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// The built-in stranger default.
    Default,
    SystemConfig(PathBuf),
    SystemProfile { name: String, file: PathBuf },
    UserConfig(PathBuf),
    UserProfile { name: String, file: PathBuf },
    /// A daemon request's `opts`.
    Request,
    /// The named environment variable.
    Env(&'static str),
    /// A command-line override (`--set key=value`, `--safety ...`).
    Cli,
    /// A policy lock in this file.
    PolicyLock(PathBuf),
    /// Clamped to a policy ceiling in this file.
    PolicyCeiling(PathBuf),
    /// Computed from another setting (`explain` from `ui.show_reading`).
    Derived(&'static str),
}

impl Source {
    fn rank(&self) -> u8 {
        match self {
            Source::Default | Source::Derived(_) => 0,
            Source::SystemConfig(_) => 1,
            Source::SystemProfile { .. } => 2,
            Source::UserConfig(_) => 3,
            Source::UserProfile { .. } => 4,
            Source::Request => 5,
            Source::Env(_) => 6,
            Source::Cli => 7,
            Source::PolicyLock(_) | Source::PolicyCeiling(_) => 8,
        }
    }

    /// True for values the user (not the defaults or the policy) chose.
    pub fn is_user(&self) -> bool {
        (1..=7).contains(&self.rank())
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::SystemConfig(p) => write!(f, "system config {}", p.display()),
            Source::SystemProfile { name, file } => write!(f, "[profile.{name}] in {}", file.display()),
            Source::UserConfig(p) => write!(f, "user config {}", p.display()),
            Source::UserProfile { name, file } => write!(f, "[profile.{name}] in {}", file.display()),
            Source::Request => f.write_str("request"),
            Source::Env(v) => write!(f, "env {v}"),
            Source::Cli => f.write_str("command line"),
            Source::PolicyLock(p) => write!(f, "locked by policy {}", p.display()),
            Source::PolicyCeiling(p) => write!(f, "clamped by policy {}", p.display()),
            Source::Derived(k) => write!(f, "from {k}"),
        }
    }
}

/// Which config files to read (`--config`, `--no-config`, `EMOTICOND_CONFIG`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConfigChoice {
    /// The system configs and the user's `config.toml`.
    #[default]
    Default,
    /// The system configs, then this file instead of the user's.
    File(PathBuf),
    /// No config files at all (the policy still applies).
    None,
}

/// Builds a [`Resolved`] configuration from files, env and overrides.
#[derive(Debug, Clone, Default)]
pub struct Loader {
    paths: Option<Paths>,
    env: Option<Env>,
    frontend: Option<String>,
    profile: Option<String>,
    config: Option<ConfigChoice>,
    user_text: Option<String>,
    system_text: Option<String>,
    policy_text: Option<String>,
    cli: Vec<String>,
}

impl Loader {
    /// Read the process environment and the platform paths.
    pub fn new() -> Loader {
        Loader::default()
    }

    /// Nothing from the process: paths under `root` ([`Paths::rooted`]) and
    /// an empty environment. For tests and sandboxed embedders.
    pub fn isolated(root: impl AsRef<Path>) -> Loader {
        Loader { paths: Some(Paths::rooted(root)), env: Some(Env::empty()), ..Loader::default() }
    }

    /// Use these paths instead of detecting them.
    pub fn paths(mut self, paths: Paths) -> Loader {
        self.paths = Some(paths);
        self
    }

    /// Use this environment instead of the process's.
    pub fn env(mut self, env: Env) -> Loader {
        self.env = Some(env);
        self
    }

    /// The front-end's name (`cli`, `quickshell`, `anyrun`): its
    /// `[profile.<name>]` applies unless `EMOTICOND_PROFILE` or
    /// [`profile`](Loader::profile) names another.
    pub fn frontend(mut self, name: impl Into<String>) -> Loader {
        self.frontend = Some(name.into());
        self
    }

    /// `--profile NAME`: beats `EMOTICOND_PROFILE` and the front-end name.
    pub fn profile(mut self, name: impl Into<String>) -> Loader {
        self.profile = Some(name.into());
        self
    }

    /// `--config PATH`: read this file instead of the user's config.toml.
    pub fn config_file(mut self, path: impl Into<PathBuf>) -> Loader {
        self.config = Some(ConfigChoice::File(path.into()));
        self
    }

    /// `--no-config`: read no config files (the policy still applies).
    pub fn no_config(mut self) -> Loader {
        self.config = Some(ConfigChoice::None);
        self
    }

    /// Use this text as the user config instead of reading the file.
    pub fn user_toml(mut self, text: impl Into<String>) -> Loader {
        self.user_text = Some(text.into());
        self
    }

    /// Use this text as the (single) system config instead of reading files.
    pub fn system_toml(mut self, text: impl Into<String>) -> Loader {
        self.system_text = Some(text.into());
        self
    }

    /// Use this text as the policy instead of reading the policy file.
    pub fn policy_toml(mut self, text: impl Into<String>) -> Loader {
        self.policy_text = Some(text.into());
        self
    }

    /// A command-line override, `key=value` (`safety=moderate`,
    /// `styles.lenny=hide`, `search.min_quality=4.5`, `max_len=none`).
    /// Checked by [`load`](Loader::load).
    pub fn set(mut self, kv: impl Into<String>) -> Loader {
        self.cli.push(kv.into());
        self
    }

    /// Several overrides at once.
    pub fn set_all<I: IntoIterator<Item = S>, S: Into<String>>(mut self, kvs: I) -> Loader {
        self.cli.extend(kvs.into_iter().map(Into::into));
        self
    }

    /// Read everything and resolve. Fails only on bad command-line input
    /// (an unknown key, a bad value, an unreadable `--config` file).
    pub fn load(self) -> Result<Resolved, ConfigError> {
        let env = self.env.unwrap_or_else(Env::from_process);
        let paths = self.paths.unwrap_or_else(|| Paths::detect_with(&env));
        let home = paths.home.clone();
        let mut warnings = Vec::new();

        // Env layer (also yields the config choice, profile and data dirs).
        let ev = env_layer(&env, &paths, &mut warnings);

        // Policy.
        let policy = match self.policy_text {
            Some(t) => Some(parse_policy(&t, &paths.policy_file, &mut warnings)),
            None => match std::fs::read_to_string(&paths.policy_file) {
                Ok(t) => Some(parse_policy(&t, &paths.policy_file, &mut warnings)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    warnings.push(Warning::new(
                        codes::POLICY_UNREADABLE,
                        None,
                        format!("{}: cannot read policy ({e}); applying a fail-closed policy", paths.policy_file.display()),
                    ));
                    Some(PolicyFile::fail_closed(Some(&paths.policy_file)))
                }
            },
        };

        // Config files.
        let cli_choice = self.config.is_some();
        let choice = self.config.or(ev.config.clone()).unwrap_or_default();
        let mut system_files = Vec::new(); // lowest priority first
        let mut user_file = None;
        let mut files_read = Vec::new();
        if choice != ConfigChoice::None {
            if let Some(t) = &self.system_text {
                let p = paths.system_configs().into_iter().next().unwrap_or_else(|| PathBuf::from("system config"));
                if let Some(f) = parse_config(t, &p, home.as_deref(), true, &mut warnings) {
                    system_files.push(f);
                }
                files_read.push(p);
            } else {
                for p in paths.system_configs().into_iter().rev() {
                    if let Some(t) = read_optional(&p, &mut warnings) {
                        if let Some(f) = parse_config(&t, &p, home.as_deref(), true, &mut warnings) {
                            system_files.push(f);
                        }
                        files_read.push(p);
                    }
                }
            }
            let (path, text) = match (&choice, &self.user_text) {
                (ConfigChoice::File(p), None) => match std::fs::read_to_string(p) {
                    Ok(t) => (p.clone(), Some(t)),
                    Err(source) if cli_choice => return Err(ConfigError::Io { path: p.clone(), source }),
                    Err(e) => {
                        warnings.push(Warning::new(
                            codes::CONFIG_UNREADABLE,
                            None,
                            format!("{}: cannot read config named by EMOTICOND_CONFIG ({e}); skipped", p.display()),
                        ));
                        (p.clone(), None)
                    }
                },
                (ConfigChoice::File(p), Some(t)) => (p.clone(), Some(t.clone())),
                (_, Some(t)) => (paths.user_config(), Some(t.clone())),
                (_, None) => {
                    let p = paths.user_config();
                    let t = read_optional(&p, &mut warnings);
                    (p, t)
                }
            };
            if let Some(t) = text {
                match walk::config_file(&t, &path, home.as_deref(), false, &mut warnings) {
                    Ok(f) => user_file = Some(f),
                    Err(message) if cli_choice && self.user_text.is_none() => return Err(ConfigError::Parse { path, message }),
                    Err(message) => warnings.push(Warning::new(
                        codes::CONFIG_UNREADABLE,
                        None,
                        format!("{}: cannot parse ({message}); skipped", path.display()),
                    )),
                }
                if user_file.is_some() {
                    files_read.push(path);
                }
            }
        }

        // Profile.
        let explicit = self.profile.clone().or(ev.profile.clone());
        let profile = explicit.clone().or(self.frontend.clone());
        let mut layers: Vec<Entry> = Vec::new();
        for f in &system_files {
            layers.extend(f.top.iter().cloned());
        }
        let mut found_profile = false;
        if let Some(name) = &profile {
            for f in &system_files {
                if let Some(e) = f.profiles.get(name) {
                    found_profile = true;
                    layers.extend(e.iter().cloned());
                }
            }
        }
        if let Some(f) = &user_file {
            layers.extend(f.top.iter().cloned());
            if let Some(e) = profile.as_ref().and_then(|n| f.profiles.get(n)) {
                found_profile = true;
                layers.extend(e.iter().cloned());
            }
        }
        if let (Some(name), false, false) = (&explicit, found_profile, choice == ConfigChoice::None) {
            warnings.push(Warning::new(
                codes::UNKNOWN_PROFILE,
                Some("profile"),
                format!("no config file has a [profile.{name}]; using the top-level settings"),
            ));
        }
        layers.extend(ev.entries);

        // CLI overrides.
        for kv in &self.cli {
            let (k, v) = kv.split_once('=').ok_or_else(|| ConfigError::Syntax(kv.clone()))?;
            let key = canonical_key(k).ok_or_else(|| ConfigError::UnknownKey { key: k.trim().to_string() })?;
            let spec = schema::spec(key).expect("canonical keys have specs");
            let p = spec
                .kind
                .parse_str(v, Ctx { home: home.as_deref(), base: None })
                .map_err(|b| ConfigError::InvalidValue { key: key.to_string(), message: b.message().to_string() })?;
            if let Some(note) = p.clamped {
                warnings.push(Warning::new(codes::OUT_OF_RANGE, Some(key), format!("command line: `{key}`: {note}")));
            }
            layers.push(Entry { key, value: p.value, source: Source::Cli });
        }

        let mut r = Resolved {
            config: Config::default(),
            search: SearchOptions::default(),
            policy: Policy::default(),
            locks: Locks::none(),
            paths,
            profile,
            files: files_read,
            policy_file: policy,
            env_data_dirs: ev.data_dirs,
            data_dirs: Vec::new(),
            overlays: Vec::new(),
            blocklist_files: Vec::new(),
            sources: BTreeMap::new(),
            warnings,
            layers,
        };
        r.compute(&[]);
        Ok(r)
    }
}

fn parse_policy(text: &str, path: &Path, warnings: &mut Vec<Warning>) -> PolicyFile {
    let (pf, w) = PolicyFile::parse(text, Some(path));
    warnings.extend(w);
    pf
}

fn read_optional(p: &Path, warnings: &mut Vec<Warning>) -> Option<String> {
    match std::fs::read_to_string(p) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warnings.push(Warning::new(codes::CONFIG_UNREADABLE, None, format!("{}: cannot read ({e}); skipped", p.display())));
            None
        }
    }
}

fn parse_config(text: &str, p: &Path, home: Option<&Path>, system: bool, warnings: &mut Vec<Warning>) -> Option<walk::FileLayers> {
    match walk::config_file(text, p, home, system, warnings) {
        Ok(f) => Some(f),
        Err(message) => {
            warnings.push(Warning::new(codes::CONFIG_UNREADABLE, None, format!("{}: cannot parse ({message}); skipped", p.display())));
            None
        }
    }
}

struct EnvOut {
    entries: Vec<Entry>,
    data_dirs: Vec<PathBuf>,
    config: Option<ConfigChoice>,
    profile: Option<String>,
}

/// The env vars (options.md §7.2).
fn env_layer(env: &Env, paths: &Paths, warnings: &mut Vec<Warning>) -> EnvOut {
    let mut out = EnvOut { entries: Vec::new(), data_dirs: Vec::new(), config: None, profile: None };
    let ctx = Ctx { home: paths.home.as_deref(), base: None };

    if let Some(c) = env.get_nonempty("EMOTICOND_CONFIG") {
        out.config = Some(if matches!(c.trim().to_ascii_lowercase().as_str(), "none" | "off") {
            ConfigChoice::None
        } else {
            ConfigChoice::File(crate::paths::expand(c.trim(), ctx.home, None))
        });
    }
    out.profile = env.get_nonempty("EMOTICOND_PROFILE").map(|s| s.trim().to_string());

    if let Some(d) = env.get_nonempty("EMOTICOND_DATA") {
        out.data_dirs = paths.platform.split_list(d).iter().map(|p| crate::paths::expand(&p.to_string_lossy(), ctx.home, None)).collect();
    }
    for (var, key) in [
        ("EMOTICOND_PICK_LOG", "dev.pick_log"),
        ("EMOTICOND_REPORT_ENDPOINT", "feedback.endpoint"),
        ("EMOTICOND_IDLE_EXIT", "daemon.idle_exit"),
        ("EMOTICOND_TUNING_WEIGHTS", "advanced.tuning.weights"),
    ] {
        let Some(raw) = env.get(var) else { continue };
        let kind = schema::spec(key).expect("known key").kind;
        match kind.parse_str(raw, ctx) {
            Ok(p) => {
                if let Some(note) = p.clamped {
                    warnings.push(Warning::new(codes::OUT_OF_RANGE, Some(key), format!("${var}: {note}")));
                }
                out.entries.push(Entry { key, value: p.value, source: Source::Env(var) });
            }
            Err(Bad::Type(m) | Bad::Enum(m)) => {
                warnings.push(Warning::new(codes::INVALID_VALUE, Some(key), format!("${var}: {m}; ignored")))
            }
        }
    }
    out
}

/// The effective configuration: what to open the database with, the
/// default per-query options, the policy, and everything the state crate
/// and the front-end need.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Resolved {
    /// The typed settings after every layer and the policy, with default
    /// paths filled in (`popularity.store`, `feedback.queue`).
    pub config: Config,
    /// The default per-query options (also `open_options().defaults`).
    /// `usage_weight` follows `popularity.weight` (`off` when popularity is
    /// off); `explain` follows `ui.show_reading` unless set.
    pub search: SearchOptions,
    /// The core policy (also `open_options().policy`).
    pub policy: Policy,
    /// Locked settings and ceilings, for settings pages.
    pub locks: Locks,
    /// Every resolved location.
    pub paths: Paths,
    /// The profile in effect, if any.
    pub profile: Option<String>,
    /// Config files that were read, lowest priority first.
    pub files: Vec<PathBuf>,
    /// The policy file, if one exists.
    pub policy_file: Option<PolicyFile>,
    /// `EMOTICOND_DATA`, searched first.
    pub env_data_dirs: Vec<PathBuf>,
    /// The data search path: env dirs, `data.dirs`, then the platform dirs.
    pub data_dirs: Vec<PathBuf>,
    /// Overlay paths, in application order: the state overlay dir, the
    /// user overlay dir (each only if it exists), then `data.overlays`.
    pub overlays: Vec<PathBuf>,
    /// Blocklist files to read (the state crate reads them; the core takes
    /// face ids): the state blocklist, the system blocklist, the policy's
    /// `[data] blocklist`, then `data.blocklist`.
    pub blocklist_files: Vec<PathBuf>,
    /// Everything worth telling the user, in the order found. See
    /// [`crate::level`].
    pub warnings: Vec<Warning>,
    sources: BTreeMap<&'static str, Source>,
    layers: Vec<Entry>,
}

/// Per-query options for one daemon request.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RequestOptions {
    pub options: SearchOptions,
    /// Unknown keys and clamped values in the request.
    pub warnings: Vec<Warning>,
}

impl Resolved {
    /// Merge the layers (plus `extra`, a request layer) into `self`.
    fn compute(&mut self, extra: &[Entry]) -> Vec<Warning> {
        let mut new_warnings = Vec::new();
        let mut all: Vec<&Entry> = self.layers.iter().chain(extra).collect();
        all.sort_by_key(|e| e.source.rank()); // stable: file order within a layer
        let mut cfg = Config::default();
        let mut sources: BTreeMap<&'static str, Source> = SPECS.iter().map(|s| (s.key, Source::Default)).collect();
        for e in all {
            schema::set(&mut cfg, e.key, &e.value);
            sources.insert(e.key, e.source.clone());
        }

        // Policy: locks, then ceilings.
        let (policy, locks, ppath) = match &self.policy_file {
            Some(pf) => (pf.policy.clone(), pf.locks(), pf.path.clone().unwrap_or_default()),
            None => (Policy::default(), Locks::none(), PathBuf::new()),
        };
        if let Some(pf) = &self.policy_file {
            for (k, v) in &pf.locks {
                let cur = schema::get(&cfg, k);
                if cur != *v && sources[k].is_user() {
                    new_warnings.push(Warning::new(
                        codes::LOCKED,
                        Some(k),
                        format!("`{k}` is set by your system administrator to {v}; your {cur} ({}) is not used", sources[k]),
                    ));
                }
                schema::set(&mut cfg, k, v);
                sources.insert(k, Source::PolicyLock(ppath.clone()));
            }
        }
        if let Some(max) = policy.max_safety {
            if cfg.search.safety > max {
                let k = "search.safety";
                new_warnings.push(Warning::new(
                    codes::CLAMPED,
                    Some(k),
                    format!("`{k}` {} ({}) is looser than the system allows; using {}", schema::get(&cfg, k), sources[k], {
                        let mut c = cfg.clone();
                        c.search.safety = max;
                        schema::get(&c, k)
                    }),
                ));
                cfg.search.safety = max;
                sources.insert(k, Source::PolicyCeiling(ppath.clone()));
            }
        }
        let capped = policy.popularity(cfg.popularity.mode);
        if capped != cfg.popularity.mode {
            let k = "popularity.mode";
            let before = schema::get(&cfg, k);
            cfg.popularity.mode = capped;
            new_warnings.push(Warning::new(
                codes::CLAMPED,
                Some(k),
                format!("`{k}` {before} ({}) is above the system's ceiling; using {}", sources[k], schema::get(&cfg, k)),
            ));
            sources.insert(k, Source::PolicyCeiling(ppath.clone()));
        }

        // Derived values and default paths.
        if cfg.ui.show_reading && sources["search.explain"] == Source::Default {
            cfg.search.explain = Explain::Reading;
            sources.insert("search.explain", Source::Derived("ui.show_reading"));
        }
        if cfg.popularity.store.is_none() {
            cfg.popularity.store = Some(self.paths.usage_file());
        }
        if cfg.feedback.queue.is_none() {
            cfg.feedback.queue = Some(self.paths.reports_queue());
        }
        cfg.search.usage_weight =
            if cfg.popularity.mode == PopularityMode::Off { UsageWeight::Off } else { cfg.popularity.weight };

        // The per-query defaults.
        let mut search = cfg.search.clone();
        if let Some(w) = cfg.tuning_weights {
            #[cfg(feature = "unstable-tuning")]
            {
                let mut t = emoticond::Tuning::default();
                t.weights = Some(w);
                search.tuning = Some(t);
            }
            #[cfg(not(feature = "unstable-tuning"))]
            new_warnings.push(Warning::new(
                codes::TUNING_IGNORED,
                Some("advanced.tuning.weights"),
                format!("tuning weights {w:?} ({}) ignored: built without the unstable-tuning feature", sources["advanced.tuning.weights"]),
            ));
        }
        policy.clamp(&mut search);
        new_warnings.extend(search.validate());

        // Locations.
        let mut data_dirs = self.env_data_dirs.clone();
        data_dirs.extend(cfg.data.dirs.iter().cloned());
        data_dirs.extend(self.paths.data_dirs.iter().cloned());
        dedup(&mut data_dirs);
        let mut overlays: Vec<PathBuf> =
            [self.paths.state_overlays_dir(), self.paths.user_overlays_dir()].into_iter().filter(|p| p.is_dir()).collect();
        overlays.extend(cfg.data.overlays.iter().cloned());
        dedup(&mut overlays);
        let mut blocklist_files = vec![self.paths.state_blocklist(), self.paths.system_blocklist.clone()];
        if let Some(pf) = &self.policy_file {
            blocklist_files.extend(pf.blocklist.iter().cloned());
        }
        blocklist_files.extend(cfg.data.blocklist.iter().cloned());
        dedup(&mut blocklist_files);

        self.config = cfg;
        self.search = search;
        self.policy = policy;
        self.locks = locks;
        self.data_dirs = data_dirs;
        self.overlays = overlays;
        self.blocklist_files = blocklist_files;
        self.sources = sources;
        if extra.is_empty() {
            self.warnings.extend(new_warnings.iter().cloned());
        }
        new_warnings
    }

    /// What to open the database with: data dirs, data set, overlays,
    /// policy and the default search options. `blocklist` is left empty:
    /// the state crate reads [`blocklist_files`](Resolved::blocklist_files)
    /// into face ids.
    pub fn open_options(&self) -> OpenOptions {
        let mut o = OpenOptions::default();
        o.data_dirs = self.data_dirs.clone();
        o.dataset = self.config.data.dataset;
        o.overlays = self.overlays.iter().cloned().map(OverlaySource::Path).collect();
        o.policy = self.policy.clone();
        o.defaults = self.search.clone();
        o
    }

    /// The effective popularity mode (after the policy ceiling).
    pub fn popularity_mode(&self) -> PopularityMode {
        self.config.popularity.mode
    }

    /// True when the user and the policy both allow sending reports.
    /// Reports still need an endpoint before anything leaves the machine.
    pub fn send_reports(&self) -> bool {
        self.reports_choice().send() && !self.policy.reports_disabled
    }

    /// Where sending stands, before the policy: `feedback.send` when a file,
    /// the environment or a flag set it, else the saved answer
    /// (`consent.json`), else unasked.
    pub fn reports_choice(&self) -> crate::ReportsChoice {
        if !matches!(self.source("feedback.send"), None | Some(Source::Default)) {
            return crate::ReportsChoice::Set(self.config.feedback.send);
        }
        match crate::consent::read(&self.paths.consent_file()) {
            Some(c) => crate::ReportsChoice::Answered(c),
            None => crate::ReportsChoice::Unasked,
        }
    }

    /// Where the effective value of `key` came from.
    pub fn source(&self, key: &str) -> Option<&Source> {
        canonical_key(key).and_then(|k| self.sources.get(k))
    }

    /// The effective value of `key` (any accepted spelling).
    pub fn value(&self, key: &str) -> Option<Value> {
        canonical_key(key).map(|k| schema::get(&self.config, k))
    }

    /// The options for one daemon request: `opts` (SearchOptions names,
    /// snake_case, nested `styles`/`emotions`) layered between the profile
    /// and env (options.md §7.2), then clamped by the policy. `offset`,
    /// `seed`, `exclude` and `usage` pass straight through. Unknown keys
    /// are warnings; bad values are errors (options.md §10.1).
    pub fn with_request(&self, opts: &serde_json::Value) -> Result<RequestOptions, ConfigError> {
        let serde_json::Value::Object(obj) = opts else {
            return Err(ConfigError::InvalidValue { key: "opts".into(), message: "expected an object".into() });
        };
        let mut w = Walker::new(Ctx { home: self.paths.home.as_deref(), base: None }, "request: ".into(), true, Source::Request);
        let mut offset = None;
        let mut seed = None;
        let mut exclude = None;
        let mut usage = None;
        let bad = |key: &str, message: String| ConfigError::InvalidValue { key: key.into(), message };
        for (k, v) in obj {
            match k.as_str() {
                "offset" => {
                    offset = Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(k, "expected a non-negative integer".into()))?)
                }
                "seed" => seed = Some(v.as_u64().ok_or_else(|| bad(k, "expected a non-negative integer".into()))?),
                "exclude" => {
                    let arr = v.as_array().ok_or_else(|| bad(k, "expected a list of face ids".into()))?;
                    let mut set = BTreeSet::new();
                    for x in arr {
                        let s = x.as_str().ok_or_else(|| bad(k, "expected a list of face ids".into()))?;
                        set.insert(s.parse::<FaceId>().map_err(|e| bad(k, e.to_string()))?);
                    }
                    exclude = Some(set);
                }
                "usage" => usage = Some(serde_json::from_value::<UsageMap>(v.clone()).map_err(|e| bad(k, e.to_string()))?),
                "usage_weight" => w.value("popularity.weight", &walk::json_to_toml(v)),
                "tuning" => w.value("advanced.tuning", &walk::json_to_toml(v)),
                _ => w.value(&format!("search.{k}"), &walk::json_to_toml(v)),
            }
        }
        let (entries, mut warnings, errors) = w.finish();
        if let Some(e) = errors.into_iter().next() {
            return Err(e);
        }
        let mut tmp = self.clone();
        warnings.extend(tmp.compute(&entries).into_iter().filter(|w| entries.iter().any(|e| Some(e.key) == w.key.as_deref())));
        let mut options = tmp.search;
        if let Some(o) = offset {
            options.offset = o;
        }
        if let Some(s) = seed {
            options.seed = s;
        }
        if let Some(e) = exclude {
            options.exclude = e;
        }
        if let Some(u) = usage {
            options.usage = u;
            self.policy.clamp(&mut options);
        }
        Ok(RequestOptions { options, warnings })
    }

    /// Every setting with its effective value and where it came from, plus
    /// the resolved locations (for `emoticond config show`).
    pub fn describe(&self) -> Description {
        Description {
            settings: SPECS
                .iter()
                .map(|s| DescribedSetting {
                    key: s.key,
                    value: schema::get(&self.config, s.key),
                    source: self.sources.get(s.key).cloned().unwrap_or(Source::Default),
                    locked: self.locks.locked(s.key),
                    page: s.page,
                })
                .collect(),
            profile: self.profile.clone(),
            files: self.files.clone(),
            policy_file: self.policy_file.as_ref().and_then(|p| p.path.clone()),
            data_dirs: self.data_dirs.clone(),
            overlays: self.overlays.clone(),
            blocklist_files: self.blocklist_files.clone(),
            state_dir: self.paths.state_dir.clone(),
            send_reports: self.send_reports(),
            warnings: self.warnings.clone(),
        }
    }
}

/// One line of [`Description`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DescribedSetting {
    pub key: &'static str,
    pub value: Value,
    pub source: Source,
    pub locked: bool,
    pub page: Page,
}

/// The effective configuration with provenance. `Display` renders it as
/// commented TOML-ish lines.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Description {
    pub settings: Vec<DescribedSetting>,
    pub profile: Option<String>,
    pub files: Vec<PathBuf>,
    pub policy_file: Option<PathBuf>,
    pub data_dirs: Vec<PathBuf>,
    pub overlays: Vec<PathBuf>,
    pub blocklist_files: Vec<PathBuf>,
    pub state_dir: PathBuf,
    pub send_reports: bool,
    pub warnings: Vec<Warning>,
}

impl fmt::Display for Description {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |v: &[PathBuf]| Value::Paths(v.to_vec()).to_string();
        writeln!(f, "# profile: {}", self.profile.as_deref().unwrap_or("(none)"))?;
        writeln!(f, "# config files: {}", if self.files.is_empty() { "(none)".into() } else { list(&self.files) })?;
        writeln!(f, "# policy: {}", self.policy_file.as_ref().map_or("(none)".into(), |p| p.display().to_string()))?;
        writeln!(f, "# data dirs: {}", list(&self.data_dirs))?;
        writeln!(f, "# overlays: {}", list(&self.overlays))?;
        writeln!(f, "# blocklists: {}", list(&self.blocklist_files))?;
        writeln!(f, "# state dir: {}", self.state_dir.display())?;
        writeln!(f, "# reports sent: {}", if self.send_reports { "yes, once an endpoint exists" } else { "no (saved on this computer only)" })?;
        let width = self.settings.iter().map(|s| s.key.len() + s.value.to_string().len()).max().unwrap_or(0) + 3;
        for s in &self.settings {
            let line = format!("{} = {}", s.key, s.value);
            writeln!(f, "{line:<width$} # {}{}", s.source, if s.locked { " [locked]" } else { "" })?;
        }
        for w in &self.warnings {
            writeln!(f, "# {:?}: {}", crate::level(w), w.message)?;
        }
        Ok(())
    }
}
