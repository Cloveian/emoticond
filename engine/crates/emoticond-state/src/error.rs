//! Errors. Problems that should not stop a front-end (a bad blocklist line,
//! an unreadable usage file) are [`emoticond::Warning`]s instead.

use crate::feedback::{SendError, SendOff};
use std::path::{Path, PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading, writing, locking or renaming a file failed.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Serialising a record failed (should not happen with the core types).
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// Neither `$XDG_STATE_HOME` nor `$HOME` is set (absolute).
    #[error("no state directory: neither XDG_STATE_HOME nor HOME is set to an absolute path")]
    NoStateDir,
    /// Sending reports is switched off, by the user, the packager's policy,
    /// or because this build has no network support.
    #[error("sending reports is turned off: {0}")]
    SendingOff(SendOff),
    /// The sender failed; the reports stay queued.
    #[error(transparent)]
    Send(#[from] SendError),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Error {
        Error::Io { path: path.to_path_buf(), source }
    }

    /// True when sending is off by policy (the packager's lock), as opposed to
    /// a user setting or a transport failure.
    pub fn is_sending_locked(&self) -> bool {
        matches!(self, Error::SendingOff(SendOff::Policy | SendOff::NotBuilt))
    }
}
