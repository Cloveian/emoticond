//! The user's choices about what leaves the machine (`<state dir>/consent.json`).
//!
//! - **reports**: sending the reports they make from the report menus. On
//!   by default (clicking a report item is the act of reporting);
//!   `emoticond reports off` turns it off.
//! - **stats**: anonymous usage statistics (bucketed pick counts and a
//!   once-a-day "in use" upload, docs/collector.md). Off until the user says
//!   yes: `emoticond` asks once on first interactive use, a front-end can ask
//!   in its own UI (the `consent` protocol op), and `emoticond stats on|off`
//!   changes it later.
//!
//! A value set in a config file, the environment or on the command line
//! (`feedback.send`, `popularity.mode`) wins over the saved answer. Turning
//! something on only sends what is made from then on (`since_ms`).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// One saved answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    pub send: bool,
    /// When the answer was given (unix ms): only what is made at or after
    /// this is sent.
    pub since_ms: u64,
}

/// What `consent.json` holds; a missing topic was never answered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Consents {
    pub reports: Option<Consent>,
    pub stats: Option<Consent>,
}

/// The two things a user can say yes or no to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Reports,
    Stats,
}

/// Where one topic stands, before the packager's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Set in a config file, the environment or a flag.
    Set(bool),
    /// The user's saved answer.
    Answered(Consent),
    /// Never answered, and this is the default (reports: on).
    Default(bool),
    /// Never answered, and the user should be asked (stats): off until then.
    Unasked,
}

impl Choice {
    pub fn send(self) -> bool {
        match self {
            Choice::Set(b) | Choice::Default(b) => b,
            Choice::Answered(c) => c.send,
            Choice::Unasked => false,
        }
    }

    /// Nothing made before this is sent.
    pub fn since_ms(self) -> u64 {
        match self {
            Choice::Answered(c) => c.since_ms,
            _ => 0,
        }
    }
}

/// The saved answers; empty when there are none or the file is unreadable.
pub fn read(path: &Path) -> Consents {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save one answer, keeping the other (written beside and renamed in).
pub fn write(path: &Path, topic: Topic, consent: Consent) -> std::io::Result<()> {
    let mut all = read(path);
    match topic {
        Topic::Reports => all.reports = Some(consent),
        Topic::Stats => all.stats = Some(consent),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&all)?)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_saved_separately() {
        let dir = std::env::temp_dir().join(format!("emoticond-consent-{}", std::process::id()));
        let p = dir.join("consent.json");
        assert_eq!(read(&p), Consents::default());
        let yes = Consent { send: true, since_ms: 5 };
        let no = Consent { send: false, since_ms: 7 };
        write(&p, Topic::Stats, yes).unwrap();
        write(&p, Topic::Reports, no).unwrap();
        assert_eq!(read(&p), Consents { reports: Some(no), stats: Some(yes) });
        assert!(Choice::Answered(yes).send());
        assert_eq!(Choice::Answered(yes).since_ms(), 5);
        assert!(!Choice::Unasked.send());
        assert!(Choice::Default(true).send());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
