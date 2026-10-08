//! Packager / admin policy (docs/options.md §7.3).
//!
//! A policy has **ceilings** (an ordered setting may not be looser than X)
//! and **locks** (a setting has a fixed value). The config crate reads it
//! from `/etc/emoticond/policy.toml`; the library gets it as
//! `OpenOptions::policy` and applies [`Policy::clamp`] to every query, so a
//! front-end that skips the config crate still can't loosen it. Options that
//! would cross a ceiling are clamped, never rejected, and the result lists
//! their names in `clamped`.

use crate::options::{named, SearchOptions, Safety, StyleMode, UsageWeight};
use serde::{Deserialize, Serialize};

/// Popularity modes (options.md §4). The library only sees the effect (a
/// usage map or none); the mode is here so the config and state crates and
/// the policy share one type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopularityMode {
    /// No usage map; nothing recorded.
    Off,
    /// Picks recorded and used on this machine only (default).
    #[default]
    Local,
    /// As local, plus anonymised counts queued for upload.
    Shared,
}

named!(PopularityMode { Off => "off", Local => "local", Shared => "shared" });

/// Fixed values for the face styles (`[locks] styles.*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct StyleLocks {
    pub lenny: Option<StyleMode>,
    pub crude: Option<StyleMode>,
    pub long: Option<StyleMode>,
}

/// Ceilings and locks. `Policy::default()` is no policy: everything open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Policy {
    /// Ceiling: the loosest safety a query may use (`safety.min = "strict"`
    /// in policy.toml means `max_safety = Strict`).
    pub max_safety: Option<Safety>,
    /// Lock: every query uses exactly this safety (applied after the
    /// ceiling).
    pub safety: Option<Safety>,
    /// Ceiling on popularity (`popularity.max`). `Off` also makes the library
    /// ignore any usage map a query brings.
    pub max_popularity: Option<PopularityMode>,
    /// Lock: reports are never sent (`feedback.send = false`). They are still
    /// created and queued, and local effects still apply.
    pub reports_disabled: bool,
    /// The report endpoint to use instead of the compiled-in one (an
    /// organisation's own collector). The library never sends; the state
    /// crate's sender reads it.
    pub report_endpoint: Option<String>,
    /// Locks on the face styles.
    pub styles: StyleLocks,
}

impl Policy {
    /// True when the policy constrains nothing.
    pub fn is_open(&self) -> bool {
        *self == Policy::default()
    }

    /// The popularity mode a user may actually have, given what they chose.
    pub fn popularity(&self, wanted: PopularityMode) -> PopularityMode {
        match self.max_popularity {
            Some(max) if wanted > max => max,
            _ => wanted,
        }
    }

    /// Bring `opts` inside the policy. Returns the names of the options it
    /// changed (as `SearchResult::clamped` reports them), in a fixed order.
    pub fn clamp(&self, opts: &mut SearchOptions) -> Vec<&'static str> {
        let mut out = Vec::new();
        let before = opts.safety;
        if let Some(max) = self.max_safety {
            if opts.safety > max {
                opts.safety = max;
            }
        }
        if let Some(s) = self.safety {
            opts.safety = s;
        }
        if opts.safety != before {
            out.push("safety");
        }
        for (name, lock, cur) in [
            ("styles.lenny", self.styles.lenny, &mut opts.styles.lenny),
            ("styles.crude", self.styles.crude, &mut opts.styles.crude),
            ("styles.long", self.styles.long, &mut opts.styles.long),
        ] {
            if let Some(v) = lock {
                if *cur != v {
                    *cur = v;
                    out.push(name);
                }
            }
        }
        if self.max_popularity == Some(PopularityMode::Off) && (!opts.usage.is_empty() || opts.usage_weight != UsageWeight::Off) {
            let had_usage = !opts.usage.is_empty();
            opts.usage = Default::default();
            opts.usage_weight = UsageWeight::Off;
            if had_usage {
                out.push("usage");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_policy_changes_nothing() {
        let p = Policy::default();
        assert!(p.is_open());
        let mut o = SearchOptions { safety: Safety::Off, ..SearchOptions::default() };
        let before = o.clone();
        assert!(p.clamp(&mut o).is_empty());
        assert_eq!(o, before);
    }

    #[test]
    fn safety_ceiling_clamps_looser_only() {
        let p = Policy { max_safety: Some(Safety::Moderate), ..Policy::default() };
        let mut o = SearchOptions { safety: Safety::Off, ..SearchOptions::default() };
        assert_eq!(p.clamp(&mut o), ["safety"]);
        assert_eq!(o.safety, Safety::Moderate);
        let mut o = SearchOptions::default(); // strict is inside the ceiling
        assert!(p.clamp(&mut o).is_empty());
        assert_eq!(o.safety, Safety::Strict);
    }

    #[test]
    fn locks_fix_values() {
        let p = Policy {
            safety: Some(Safety::Strict),
            styles: StyleLocks { crude: Some(StyleMode::Hide), ..StyleLocks::default() },
            ..Policy::default()
        };
        let mut o = SearchOptions::legacy(10);
        assert_eq!(p.clamp(&mut o), ["safety", "styles.crude"]);
        assert_eq!(o.safety, Safety::Strict);
        assert_eq!(o.styles.crude, StyleMode::Hide);
        assert_eq!(o.styles.lenny, StyleMode::Demote);
    }

    #[test]
    fn popularity_off_drops_usage() {
        let p = Policy { max_popularity: Some(PopularityMode::Off), ..Policy::default() };
        let mut o = SearchOptions::default();
        o.usage.global.insert(crate::FaceId::of_text("x"), 1.0);
        assert_eq!(p.clamp(&mut o), ["usage"]);
        assert!(o.usage.is_empty());
        assert_eq!(o.usage_weight, UsageWeight::Off);
        assert_eq!(p.popularity(PopularityMode::Shared), PopularityMode::Off);
        let p = Policy { max_popularity: Some(PopularityMode::Local), ..Policy::default() };
        assert_eq!(p.popularity(PopularityMode::Shared), PopularityMode::Local);
        assert_eq!(p.popularity(PopularityMode::Off), PopularityMode::Off);
    }

    #[test]
    fn serde_round_trip() {
        let p = Policy { max_safety: Some(Safety::Strict), reports_disabled: true, ..Policy::default() };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Policy>(&j).unwrap(), p);
        assert_eq!(serde_json::from_str::<Policy>("{}").unwrap(), Policy::default());
    }
}
