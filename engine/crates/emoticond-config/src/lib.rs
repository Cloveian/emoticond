//! emoticond-config: what the **user** and the **packager** wrote, turned into
//! the core library's [`OpenOptions`](emoticond::OpenOptions),
//! [`SearchOptions`](emoticond::SearchOptions) and [`Policy`](emoticond::Policy)
//! (docs/options.md §1, §7–§10).
//!
//! The core library is pure; this crate does the reading:
//!
//! - **Paths** ([`Paths`]): XDG dirs on Linux (platform dirs on macOS and
//!   Windows), the fixed `/etc/emoticond/policy.toml`, and the state locations
//!   the state crate writes to.
//! - **Files**: `config.toml` (system, then user, with `[profile.<name>]`
//!   tables per front-end) and `policy.toml`.
//! - **Env**: `EMOTICOND_CONFIG`, `EMOTICOND_DATA`, `EMOTICOND_PROFILE`,
//!   `EMOTICOND_PICK_LOG`, `EMOTICOND_REPORT_ENDPOINT` (`none` turns sending
//!   off), `EMOTICOND_IDLE_EXIT` and
//!   `EMOTICOND_TUNING_WEIGHTS`.
//! - **Precedence** (options.md §7.2): policy > CLI flags > env > daemon
//!   request > profile > user config > system config > defaults.
//!
//! ```no_run
//! let resolved = emoticond_config::Loader::new()
//!     .frontend("quickshell")
//!     .set("safety=moderate")
//!     .load()
//!     .expect("bad command-line option");
//! for w in &resolved.warnings {
//!     eprintln!("emoticond: {}", w.message);
//! }
//! let open = resolved.open_options(); // hand to Database::open
//! let locked = resolved.locks.locked("feedback.send"); // grey out a control
//! # let _ = (open, locked);
//! ```
//!
//! Problems in files never stop a front-end from starting: unknown keys and
//! bad values become [`Warning`](emoticond::Warning)s (see [`level`] and
//! [`codes`]). Only command-line overrides and daemon requests fail with a
//! [`ConfigError`].

pub mod config;
pub mod consent;
pub mod env;
mod load;
pub mod paths;
pub mod policy;
pub mod schema;
pub mod value;
mod walk;

pub use config::{Config, DEFAULT_REPORT_ENDPOINT, DaemonSettings, DataSettings, DevSettings, FeedbackSettings, PopularitySettings, UiSettings};
pub use consent::{Consent, ReportsChoice};
pub use env::Env;
pub use load::{ConfigChoice, DescribedSetting, Description, Loader, RequestOptions, Resolved, Source};
pub use paths::{Paths, Platform};
pub use policy::{Locks, PolicyFile};
pub use schema::{canonical_key, setting, settings, Page, SettingInfo};
pub use value::Value;

use emoticond::Warning;
use std::path::PathBuf;

/// Warning codes this crate emits (`Warning::code`).
pub mod codes {
    /// A key no setting has; ignored.
    pub const UNKNOWN_KEY: &str = "unknown_key";
    /// A value of the wrong type (ignored) or an unknown enum value (the
    /// key's default is used).
    pub const INVALID_VALUE: &str = "invalid_value";
    /// A value outside its range; clamped to the nearest valid value.
    pub const OUT_OF_RANGE: &str = "out_of_range";
    /// A setting that no longer has any effect; ignored.
    pub const OBSOLETE_KEY: &str = "obsolete_key";
    /// A config file exists but can't be read or parsed; skipped.
    pub const CONFIG_UNREADABLE: &str = "config_unreadable";
    /// `config_version`/`policy_version` newer than this reader.
    pub const NEWER_VERSION: &str = "newer_version";
    /// `EMOTICOND_PROFILE`/`--profile` named a profile no file has.
    pub const UNKNOWN_PROFILE: &str = "unknown_profile";
    /// A user value was replaced by a policy lock.
    pub const LOCKED: &str = "locked_by_policy";
    /// A user value was clamped to a policy ceiling.
    pub const CLAMPED: &str = "clamped_by_policy";
    /// Tuning was configured but this build lacks `unstable-tuning`.
    pub const TUNING_IGNORED: &str = "tuning_ignored";
    /// **Error level.** An unknown key in policy.toml `[locks]`/`[ceilings]`:
    /// the admin believes something is locked that isn't.
    pub const POLICY_UNKNOWN_KEY: &str = "policy_unknown_key";
    /// **Error level.** An invalid value in policy.toml; the strictest value
    /// is used where there is one.
    pub const POLICY_INVALID: &str = "policy_invalid";
    /// **Error level.** policy.toml exists but can't be read or parsed; a
    /// fail-closed policy applies (see [`PolicyFile::fail_closed`]).
    pub const POLICY_UNREADABLE: &str = "policy_unreadable";
}

/// How loudly to report a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Expected effects of the policy (a locked or clamped value).
    Info,
    Warning,
    /// Something an admin must fix (options.md §10.2).
    Error,
}

/// The level of a warning this crate produced (core warnings are
/// [`Level::Warning`]).
pub fn level(w: &Warning) -> Level {
    match &*w.code {
        codes::POLICY_UNKNOWN_KEY | codes::POLICY_INVALID | codes::POLICY_UNREADABLE => Level::Error,
        codes::LOCKED | codes::CLAMPED => Level::Info,
        _ => Level::Warning,
    }
}

/// A command-line override or daemon request that can't be applied. Files
/// never produce these (options.md §10.1: a broken config shouldn't stop
/// the picker from starting).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `--set foo=1` with no setting `foo`.
    #[error("unknown option `{key}`")]
    UnknownKey { key: String },
    /// A value of the wrong type or an unknown choice.
    #[error("invalid value for `{key}`: {message}")]
    InvalidValue { key: String, message: String },
    /// An override that isn't `key=value`.
    #[error("expected key=value, got `{0}`")]
    Syntax(String),
    /// `--config PATH` that can't be read.
    #[error("cannot read config file {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// `--config PATH` that isn't valid TOML.
    #[error("cannot parse config file {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
}
