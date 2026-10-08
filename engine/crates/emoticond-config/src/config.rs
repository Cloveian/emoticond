//! The typed config model: everything `config.toml` can say
//! (docs/options.md §3–§7).
//!
//! `Config::default()` is the **stranger** default (options.md §9):
//! strict safety, crude faces hidden, local popularity on, report sending on
//! (queued until an endpoint exists), the `core` data set, dev logs off.

use emoticond::{Dataset, PopularityMode, SearchOptions, UsageWeight};
use std::path::PathBuf;

/// Popularity settings (options.md §4.2). The state crate's `UsageStore`
/// takes these as arguments.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PopularitySettings {
    /// `off` | `local` (default) | `shared`. Capped by policy `popularity.max`.
    pub mode: PopularityMode,
    /// Exponential decay of each pick's weight, 1..=3650 days (default 30).
    pub half_life_days: f32,
    /// (term, face) pairs kept; lowest decayed weight evicted (default 5000).
    pub max_entries: u32,
    /// The usage file. `None` until resolved; the loader fills in
    /// `Paths::usage_file()` when the config doesn't name one.
    pub store: Option<PathBuf>,
    /// `false` keeps only global weights (no record of what was searched).
    pub remember_terms: bool,
    /// How much usage lifts a face; becomes `SearchOptions::usage_weight`.
    pub weight: UsageWeight,
    /// How often `shared` batches are queued (default 7 days).
    pub share_interval_days: u16,
    /// Where shared counts go. None until the service exists.
    pub share_endpoint: Option<String>,
}

impl Default for PopularitySettings {
    fn default() -> Self {
        PopularitySettings {
            mode: PopularityMode::Local,
            half_life_days: 30.0,
            max_entries: 5000,
            store: None,
            remember_terms: true,
            weight: UsageWeight::Normal,
            share_interval_days: 7,
            share_endpoint: None,
        }
    }
}

/// Where reports go by default: the project's collector (docs/collector.md).
pub const DEFAULT_REPORT_ENDPOINT: &str = "https://emoticond.mewo.gay/v1/reports";

/// Feedback and report settings (options.md §5.2). The state crate's
/// `FeedbackQueue` takes these as arguments.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FeedbackSettings {
    /// Show the report menus at all.
    pub menus: bool,
    /// The user's own switch, when set in a config file, the environment or
    /// a flag; otherwise the saved answer to "send reports?" decides
    /// (`consent`, `Resolved::reports_choice`), and with no answer nothing
    /// is sent. The policy can always turn it off.
    pub send: bool,
    /// Report collector ([`DEFAULT_REPORT_ENDPOINT`]); policy may set it,
    /// and an empty value means nowhere.
    pub endpoint: Option<String>,
    /// The report queue file; filled in with `Paths::reports_queue()`.
    pub queue: Option<PathBuf>,
    /// Oldest reports dropped beyond this many (default 500).
    pub queue_max: u32,
    /// Reports older than this are dropped (default 180 days).
    pub queue_max_age_days: u16,
    /// Local effects of a report (demote on "doesn't fit", pick on "great
    /// fit"). The offensive hide always applies.
    pub apply_locally: bool,
    /// Hard cap on the free-text note (default 500 chars).
    pub custom_max_chars: u16,
}

impl Default for FeedbackSettings {
    fn default() -> Self {
        FeedbackSettings {
            menus: true,
            send: false,
            endpoint: Some(DEFAULT_REPORT_ENDPOINT.to_string()),
            queue: None,
            queue_max: 500,
            queue_max_age_days: 180,
            apply_locally: true,
            custom_max_chars: 500,
        }
    }
}

/// Front-end UI settings (options.md §2.6, §7.4 `[ui]`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UiSettings {
    /// Show the one-line reading: makes `explain = "reading"` unless the
    /// config sets `search.explain` itself.
    pub show_reading: bool,
    /// Most results a front-end lists. None: the front-end's own choice.
    pub max_results: Option<u32>,
    /// How long a front-end waits for a helper to start.
    pub start_timeout_ms: u32,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings { show_reading: false, max_results: None, start_timeout_ms: 5000 }
    }
}

/// Long-running front-end settings (`[daemon]`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DaemonSettings {
    /// Exit after this many idle seconds; 0 = never (default 120).
    pub idle_exit: u32,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        DaemonSettings { idle_exit: 120 }
    }
}

/// Data set choice and extra locations (`[data]`, options.md §6).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DataSettings {
    /// Default `core`.
    pub dataset: Dataset,
    /// Searched before the platform data dirs (a dev checkout, say).
    pub dirs: Vec<PathBuf>,
    /// Overlay files or dirs applied after the default overlay dirs.
    pub overlays: Vec<PathBuf>,
    /// Extra blocklist files (face ids or face text, one per line).
    pub blocklist: Vec<PathBuf>,
}

impl Default for DataSettings {
    fn default() -> Self {
        DataSettings { dataset: Dataset::Core, dirs: Vec::new(), overlays: Vec::new(), blocklist: Vec::new() }
    }
}

/// Development logs (`[dev]`, options.md §4.4). Off by default.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct DevSettings {
    pub pick_log: Option<PathBuf>,
}

/// Everything a config file can set, typed.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Config {
    /// `[search]`: the default per-query options. Only the config-file keys
    /// are set from config (`usage`, `exclude`, `seed`, `offset` stay at
    /// their defaults; the front-end fills them per query).
    pub search: SearchOptions,
    pub popularity: PopularitySettings,
    pub feedback: FeedbackSettings,
    pub ui: UiSettings,
    pub daemon: DaemonSettings,
    pub data: DataSettings,
    pub dev: DevSettings,
    /// `[advanced.tuning] weights` (unstable): emotion, dense, engine,
    /// lexical. Reaches `SearchOptions::tuning` only with the
    /// `unstable-tuning` feature; otherwise ignored with a warning.
    pub tuning_weights: Option<[f32; 4]>,
}
