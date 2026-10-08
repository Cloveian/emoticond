//! Errors and warnings (docs/api-frontends.md §1.3).
//!
//! Opening can fail ([`OpenError`]) and building a report can fail
//! ([`ReportError`]). Searching never fails: bad option values are clamped or
//! defaulted and reported as [`Warning`]s in the result.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;

/// Why [`Database::open`](crate::Database::open) failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenError {
    /// No data dir held a data set (`searched` lists where we looked).
    #[error("no kaomoji data found (searched: {})", display_paths(searched))]
    NotFound { searched: Vec<PathBuf> },
    /// The data file is a format this library does not read (another
    /// format major, or a required section it does not know).
    #[error("unsupported data: {found} (this library reads {supported})")]
    Incompatible { found: String, supported: &'static str },
    /// A section of the data file is damaged: `section` is its tag
    /// (`"FIDS"`), or `"header"` / `"directory"`.
    #[error("damaged data file: section {section}")]
    Corrupt { section: &'static str },
    /// The host is big-endian; the data format is little-endian only.
    #[error("big-endian hosts are not supported")]
    BigEndian,
    /// Reading the data failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn display_paths(p: &[PathBuf]) -> String {
    if p.is_empty() {
        return "no data dirs given".into();
    }
    p.iter().map(|x| x.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// Why a [`ReportBuilder`](crate::ReportBuilder) refused to build.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReportError {
    /// A face-menu reason (great fit, other word, no fit, offensive) on a
    /// query report.
    #[error("this reason needs a face")]
    ReasonNeedsFace,
    /// A query-menu reason (read well, read wrong, missing) on a face report.
    #[error("this reason is about the query, not a face")]
    ReasonNeedsQuery,
    /// `Reason::Note` without a note.
    #[error("a custom report needs a note")]
    NoteRequired,
    /// The note is longer than the limit (in chars).
    #[error("the note is longer than {max} characters")]
    NoteTooLong { max: usize },
    /// `stamp()` was never called: the caller supplies the report id, time
    /// and disclaimer version (the library has no clock or RNG).
    #[error("the report has no id, time or disclaimer version; call stamp()")]
    NotStamped,
    /// A stored report's `key` doesn't match its query and target.
    #[error("the report key does not match its query and target")]
    KeyMismatch,
    /// A stored report lists more shown faces than reports carry.
    #[error("a report carries at most {max} shown faces")]
    TooManyShown { max: usize },
    /// Sending is switched off by policy. Only a sender returns this, never
    /// `build()`.
    #[error("sending reports is turned off on this system")]
    SendingLocked,
}

/// How serious a [`Warning`] is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    #[default]
    Warning,
    /// Something an admin must hear about (an unknown key in a policy
    /// file's locks, options.md §10.2), though nothing stopped.
    Error,
}

/// A non-fatal problem with a query or its options: an out-of-range value
/// that was clamped, an unknown emotion name, and so on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    #[serde(default)]
    pub level: Level,
    /// Stable machine-readable code, e.g. `"out_of_range"`, `"unknown_emotion"`.
    pub code: Cow<'static, str>,
    /// The option the warning is about, if any (`"limit"`, `"emotions.min"`).
    pub key: Option<String>,
    /// A human-readable sentence.
    pub message: String,
}

impl Warning {
    /// A warning at `Level::Warning`.
    pub fn new(code: &'static str, key: Option<&str>, message: impl Into<String>) -> Warning {
        Warning { level: Level::Warning, code: Cow::Borrowed(code), key: key.map(String::from), message: message.into() }
    }

    /// The same at another level.
    pub fn with_level(mut self, level: Level) -> Warning {
        self.level = level;
        self
    }
}

/// A string that is not one of an option enum's names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown value {value:?} (expected one of: {})", expected.join(", "))]
pub struct ParseOptionError {
    pub value: String,
    pub expected: &'static [&'static str],
}
