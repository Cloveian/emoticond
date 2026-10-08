//! The user's answer to "send reports?" (`<state dir>/consent.json`).
//!
//! Nothing is sent until the user answers: `emoticond` asks once on first
//! interactive use, a front-end can ask in its own UI (the `consent`
//! protocol op), and `emoticond reports on|off` changes it later. A
//! `feedback.send` set in a config file, the environment or on the command
//! line overrides the answer. Saying yes only sends reports made from then
//! on (`since_ms`), never ones queued before.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    /// Send reports.
    pub send: bool,
    /// When the answer was given (unix ms): only reports made at or after
    /// this are sent.
    pub since_ms: u64,
}

/// Where sending stands, before the packager's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportsChoice {
    /// `feedback.send` set in a config file, the environment or a flag.
    Set(bool),
    /// The user's answer.
    Answered(Consent),
    /// Never asked: nothing is sent.
    Unasked,
}

impl ReportsChoice {
    pub fn send(self) -> bool {
        match self {
            ReportsChoice::Set(b) => b,
            ReportsChoice::Answered(c) => c.send,
            ReportsChoice::Unasked => false,
        }
    }

    /// Reports older than this are never sent.
    pub fn since_ms(self) -> u64 {
        match self {
            ReportsChoice::Answered(c) => c.since_ms,
            _ => 0,
        }
    }
}

/// The saved answer; `None` when there is none or the file is unreadable.
pub fn read(path: &Path) -> Option<Consent> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Save an answer (written beside and renamed in).
pub fn write(path: &Path, consent: &Consent) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(consent)?)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_choice() {
        let dir = std::env::temp_dir().join(format!("emoticond-consent-{}", std::process::id()));
        let p = dir.join("consent.json");
        assert_eq!(read(&p), None);
        let c = Consent { send: true, since_ms: 5 };
        write(&p, &c).unwrap();
        assert_eq!(read(&p), Some(c));
        assert!(ReportsChoice::Answered(c).send());
        assert_eq!(ReportsChoice::Answered(c).since_ms(), 5);
        assert!(!ReportsChoice::Unasked.send());
        assert_eq!(ReportsChoice::Set(true).since_ms(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
