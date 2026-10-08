//! `policy.toml`: the packager/admin layer (docs/options.md §7.3, §7.5,
//! §10.2).
//!
//! ```toml
//! policy_version = 1
//! [ceilings]
//! safety.min     = "strict"     # users may not choose moderate/off
//! popularity.max = "local"      # no shared counts from this machine
//! [locks]
//! feedback.send  = false        # kill switch: reports queued, never sent
//! styles.crude   = "hide"
//! [endpoints]
//! feedback.endpoint = "https://feedback.example.org/emoticond"
//! [data]
//! blocklist = ["/etc/emoticond/blocklist.txt"]
//! dataset   = "core"
//! ```
//!
//! Ceilings and the core-enforced locks (safety, styles, report sending)
//! become a core [`Policy`], which goes into `OpenOptions::policy` so a
//! front-end that skips this crate still can't escape it. Any other
//! setting can be locked too (`[locks] search.dedupe = "strong"`); those
//! are enforced here only. [`Locks`] answers "is this control locked?" for
//! settings pages.
//!
//! Unknown keys in `[locks]` and `[ceilings]` and invalid values are
//! error-level warnings ([`codes::POLICY_UNKNOWN_KEY`],
//! [`codes::POLICY_INVALID`]); where a value is invalid the strictest one
//! applies. A policy file that exists but can't be parsed applies
//! [`PolicyFile::fail_closed`].

use crate::codes;
use crate::schema::{self, canonical_key};
use crate::value::{Ctx, Kind, Value};
use emoticond::{Policy, PopularityMode, Safety, StyleMode, Warning};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const POLICY_VERSION: i64 = 1;

/// A parsed policy file.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct PolicyFile {
    /// Where it was read from.
    pub path: Option<PathBuf>,
    /// `policy_version`, if given.
    pub version: Option<i64>,
    /// What the core library enforces on every query.
    pub policy: Policy,
    /// Every locked setting (canonical key → value), including the ones
    /// the core enforces and the `[endpoints]`/`[data] dataset` overrides.
    pub locks: BTreeMap<&'static str, Value>,
    /// `[data] blocklist`: extra blocklist files.
    pub blocklist: Vec<PathBuf>,
    /// True when this is the fail-closed policy for an unreadable file.
    pub failed_closed: bool,
}

fn perr(path: &str, label: &str, msg: impl std::fmt::Display) -> Warning {
    Warning::new(codes::POLICY_INVALID, Some(path), format!("{label}{msg}"))
}

impl PolicyFile {
    /// The policy used when `policy.toml` exists but can't be read or
    /// parsed: never looser than strict, no shared counts, no sending. An
    /// admin who wrote a policy must not get an open system because of a
    /// typo.
    pub fn fail_closed(path: Option<&Path>) -> PolicyFile {
        let mut locks = BTreeMap::new();
        locks.insert("feedback.send", Value::Bool(false));
        let mut policy = Policy::default();
        policy.max_safety = Some(Safety::Strict);
        policy.max_popularity = Some(PopularityMode::Local);
        policy.reports_disabled = true;
        PolicyFile {
            path: path.map(Path::to_path_buf),
            version: None,
            policy,
            locks,
            blocklist: Vec::new(),
            failed_closed: true,
        }
    }

    /// Parse policy text. Never fails: a syntax error gives
    /// [`fail_closed`](PolicyFile::fail_closed) plus an error-level
    /// warning.
    pub fn parse(text: &str, path: Option<&Path>) -> (PolicyFile, Vec<Warning>) {
        let label = path.map(|p| format!("{}: ", p.display())).unwrap_or_default();
        let mut warnings = Vec::new();
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                let e: toml::de::Error = e;
                warnings.push(Warning::new(
                    codes::POLICY_UNREADABLE,
                    None,
                    format!("{label}cannot parse policy ({}); applying a fail-closed policy", e.message()),
                ));
                return (PolicyFile::fail_closed(path), warnings);
            }
        };
        let mut pf = PolicyFile { path: path.map(Path::to_path_buf), ..PolicyFile::default() };
        let ctx = Ctx { home: None, base: path.and_then(Path::parent) };
        for (k, v) in &table {
            match (k.as_str(), v) {
                ("policy_version", toml::Value::Integer(n)) => {
                    pf.version = Some(*n);
                    if *n > POLICY_VERSION {
                        warnings.push(Warning::new(
                            codes::NEWER_VERSION,
                            Some("policy_version"),
                            format!("{label}policy_version {n} is newer than this reader ({POLICY_VERSION}); applying what it can"),
                        ));
                    }
                }
                ("ceilings", toml::Value::Table(t)) => {
                    for (path, v) in flatten("", t) {
                        pf.ceiling(&path, v, &label, &mut warnings);
                    }
                }
                ("locks", toml::Value::Table(t)) => {
                    for (path, v) in flatten("", t) {
                        pf.lock(&path, v, ctx, &label, &mut warnings);
                    }
                }
                ("endpoints", toml::Value::Table(t)) => {
                    for (path, v) in flatten("", t) {
                        match path.as_str() {
                            "feedback.endpoint" | "popularity.share_endpoint" => pf.lock(&path, v, ctx, &label, &mut warnings),
                            _ => warnings.push(Warning::new(
                                codes::POLICY_UNKNOWN_KEY,
                                Some(&path),
                                format!("{label}unknown endpoint `endpoints.{path}`; not applied"),
                            )),
                        }
                    }
                }
                ("data", toml::Value::Table(t)) => {
                    for (key, v) in t {
                        match key.as_str() {
                            "blocklist" => match Kind::PathList.parse_toml(v, ctx) {
                                Ok(p) => {
                                    if let Value::Paths(ps) = p.value {
                                        pf.blocklist.extend(ps);
                                    }
                                }
                                Err(b) => warnings.push(perr("data.blocklist", &label, format!("`data.blocklist`: {}", b.message()))),
                            },
                            "dataset" => pf.lock("data.dataset", v, ctx, &label, &mut warnings),
                            _ => warnings.push(Warning::new(
                                codes::UNKNOWN_KEY,
                                Some(&format!("data.{key}")),
                                format!("{label}unknown policy key `data.{key}`; ignored"),
                            )),
                        }
                    }
                }
                _ => warnings.push(Warning::new(codes::UNKNOWN_KEY, Some(k), format!("{label}unknown policy key `{k}`; ignored"))),
            }
        }
        (pf, warnings)
    }

    fn ceiling(&mut self, path: &str, v: &toml::Value, label: &str, warnings: &mut Vec<Warning>) {
        let s = v.as_str().unwrap_or_default().trim().to_ascii_lowercase();
        match path {
            "safety.min" | "search.safety.min" => {
                let parsed = serde_json::from_value::<Safety>(serde_json::Value::String(s.clone())).ok();
                if parsed.is_none() {
                    warnings.push(perr(path, label, format!("ceiling `{path}`: unknown value {v}; using \"strict\"")));
                }
                let new = parsed.unwrap_or(Safety::Strict);
                self.policy.max_safety = Some(self.policy.max_safety.map_or(new, |old| old.min(new)));
            }
            "popularity.max" => {
                let parsed = serde_json::from_value::<PopularityMode>(serde_json::Value::String(s.clone())).ok();
                if parsed.is_none() {
                    warnings.push(perr(path, label, format!("ceiling `{path}`: unknown value {v}; using \"off\"")));
                }
                let new = parsed.unwrap_or(PopularityMode::Off);
                self.policy.max_popularity = Some(self.policy.max_popularity.map_or(new, |old| old.min(new)));
            }
            _ => warnings.push(Warning::new(
                codes::POLICY_UNKNOWN_KEY,
                Some(path),
                format!("{label}unknown ceiling `{path}`: NOT enforced (known: safety.min, popularity.max)"),
            )),
        }
    }

    fn lock(&mut self, path: &str, v: &toml::Value, ctx: Ctx<'_>, label: &str, warnings: &mut Vec<Warning>) {
        if path == "safety.allow_opt_in" || path == "search.safety.allow_opt_in" {
            // under strict, query words never unlock
            // anything, so this lock always holds.
            warnings.push(Warning::new(
                codes::OBSOLETE_KEY,
                Some(path),
                format!("{label}`locks.{path}` has no effect: strict safety never unlocks by query words"),
            ));
            return;
        }
        let Some(key) = canonical_key(path) else {
            warnings.push(Warning::new(
                codes::POLICY_UNKNOWN_KEY,
                Some(path),
                format!("{label}unknown lock `{path}`: NOT enforced"),
            ));
            return;
        };
        let kind = schema::spec(key).map(|s| s.kind).unwrap_or(Kind::Bool);
        let value = match kind.parse_toml(v, ctx) {
            Ok(p) => p.value,
            Err(b) => {
                // Fail closed where a strictest value exists.
                let fallback = match key {
                    "search.safety" => Some(Value::Str("strict".into())),
                    k if k.starts_with("search.styles.") => Some(Value::Str("hide".into())),
                    "feedback.send" => Some(Value::Bool(false)),
                    "popularity.mode" => Some(Value::Str("off".into())),
                    _ => None,
                };
                let what = fallback.as_ref().map_or("not enforced".to_string(), |f| format!("using {f}"));
                warnings.push(perr(key, label, format!("lock `{path}`: {}; {what}", b.message())));
                match fallback {
                    Some(f) => f,
                    None => return,
                }
            }
        };
        match key {
            "search.safety" => self.policy.safety = enum_of(&value),
            "search.styles.lenny" => self.policy.styles.lenny = enum_of::<StyleMode>(&value),
            "search.styles.crude" => self.policy.styles.crude = enum_of::<StyleMode>(&value),
            "search.styles.long" => self.policy.styles.long = enum_of::<StyleMode>(&value),
            "feedback.send" => {
                if value == Value::Bool(true) {
                    // users may always turn sending off.
                    warnings.push(perr(
                        key,
                        label,
                        "`feedback.send` can only be locked to false (users may always turn sending off); not enforced",
                    ));
                    return;
                }
                self.policy.reports_disabled = true;
            }
            "popularity.mode" => {
                if let Some(m) = enum_of::<PopularityMode>(&value) {
                    self.policy.max_popularity = Some(self.policy.max_popularity.map_or(m, |old| old.min(m)));
                }
            }
            _ => {}
        }
        self.locks.insert(key, value);
    }

    /// The [`Locks`] view of this policy.
    pub fn locks(&self) -> Locks {
        Locks { values: self.locks.clone(), max_safety: self.policy.max_safety, max_popularity: self.policy.max_popularity }
    }
}

fn enum_of<T: serde::de::DeserializeOwned>(v: &Value) -> Option<T> {
    match v {
        Value::Str(s) => serde_json::from_value(serde_json::Value::String(s.clone())).ok(),
        _ => None,
    }
}

/// Flatten nested tables into dotted keys, keeping tables that are the
/// value of a map-typed setting whole.
fn flatten<'a>(prefix: &str, t: &'a toml::Table) -> Vec<(String, &'a toml::Value)> {
    let mut out = Vec::new();
    for (k, v) in t {
        let path = if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
        let map_kind = canonical_key(&path)
            .and_then(schema::spec)
            .is_some_and(|s| matches!(s.kind, Kind::EmotionMap | Kind::Weights));
        match v {
            toml::Value::Table(inner) if !map_kind => out.extend(flatten(&path, inner)),
            _ => out.push((path, v)),
        }
    }
    out
}

/// What a settings page needs to know about the policy: which controls are
/// locked (show them disabled, "Set by your system administrator") and
/// which choices are above a ceiling.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Locks {
    values: BTreeMap<&'static str, Value>,
    max_safety: Option<Safety>,
    max_popularity: Option<PopularityMode>,
}

impl Locks {
    /// No policy: nothing locked.
    pub fn none() -> Locks {
        Locks::default()
    }

    /// True when the setting has a fixed value. Accepts any key spelling
    /// ([`canonical_key`]): `safety`, `search.safety`, `styles.crude`.
    pub fn locked(&self, key: &str) -> bool {
        canonical_key(key).is_some_and(|k| self.values.contains_key(k))
    }

    /// The fixed value of a locked setting.
    pub fn value(&self, key: &str) -> Option<&Value> {
        canonical_key(key).and_then(|k| self.values.get(k))
    }

    /// Every locked setting (canonical keys).
    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.values.keys().copied()
    }

    /// The loosest allowed choice of an ordered setting with a ceiling
    /// (`search.safety`, `popularity.mode`).
    pub fn ceiling(&self, key: &str) -> Option<Value> {
        let name = |v: String| Value::Str(v);
        match canonical_key(key)? {
            "search.safety" => self.max_safety.map(|s| name(enum_name(&s))),
            "popularity.mode" => self.max_popularity.map(|m| name(enum_name(&m))),
            _ => None,
        }
    }

    /// Whether a settings page may offer `choice` for `key`: false for any
    /// other value of a locked setting and for values above a ceiling.
    pub fn allows(&self, key: &str, choice: &str) -> bool {
        let Some(k) = canonical_key(key) else { return true };
        let choice = choice.trim().to_ascii_lowercase();
        if let Some(v) = self.values.get(k) {
            return match v {
                Value::Str(s) => *s == choice,
                Value::Bool(b) => crate::value::Kind::Bool.parse_str(&choice, Ctx::default()).is_ok_and(|p| p.value == Value::Bool(*b)),
                other => other.to_string() == choice,
            };
        }
        match k {
            "search.safety" => match (self.max_safety, enum_of::<Safety>(&Value::Str(choice))) {
                (Some(max), Some(c)) => c <= max,
                _ => true,
            },
            "popularity.mode" => match (self.max_popularity, enum_of::<PopularityMode>(&Value::Str(choice))) {
                (Some(max), Some(c)) => c <= max,
                _ => true,
            },
            _ => true,
        }
    }

    /// True when nothing is locked or capped.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.max_safety.is_none() && self.max_popularity.is_none()
    }
}

fn enum_name<T: serde::Serialize>(t: &T) -> String {
    match serde_json::to_value(t) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_policy_maps_onto_core_policy() {
        let text = r#"
policy_version = 1
[ceilings]
safety.min     = "strict"
popularity.max = "local"
[locks]
safety.allow_opt_in = false
feedback.send       = false
styles.crude        = "hide"
[data]
blocklist = ["/etc/emoticond/blocklist.txt"]
dataset   = "core"
"#;
        let (pf, w) = PolicyFile::parse(text, Some(Path::new("/etc/emoticond/policy.toml")));
        assert_eq!(pf.policy.max_safety, Some(Safety::Strict));
        assert_eq!(pf.policy.max_popularity, Some(PopularityMode::Local));
        assert!(pf.policy.reports_disabled);
        assert_eq!(pf.policy.styles.crude, Some(StyleMode::Hide));
        assert_eq!(pf.blocklist, [PathBuf::from("/etc/emoticond/blocklist.txt")]);
        assert_eq!(pf.locks.keys().copied().collect::<Vec<_>>(), ["data.dataset", "feedback.send", "search.styles.crude"]);
        assert!(w.iter().all(|w| crate::level(w) != crate::Level::Error), "{w:?}");
        let l = pf.locks();
        assert!(l.locked("feedback.send") && l.locked("styles.crude") && !l.locked("safety"));
        assert!(l.allows("safety", "strict") && !l.allows("safety", "moderate"));
        assert!(l.allows("popularity.mode", "local") && !l.allows("popularity.mode", "shared"));
        assert!(!l.allows("feedback.send", "true") && l.allows("feedback.send", "false"));
        assert_eq!(l.ceiling("safety"), Some(Value::Str("strict".into())));
    }

    #[test]
    fn unknown_and_bad_entries_are_error_level_and_fail_closed() {
        let (pf, w) = PolicyFile::parse(
            "[ceilings]\nsafety.min = \"spicy\"\nquality.min = 3\n[locks]\nfrobnicate = true\nstyles.lenny = 7\nfeedback.send = true\n",
            None,
        );
        assert_eq!(pf.policy.max_safety, Some(Safety::Strict));
        assert_eq!(pf.policy.styles.lenny, Some(StyleMode::Hide));
        assert!(!pf.policy.reports_disabled, "feedback.send = true is not a lock");
        let mut errors: Vec<_> = w.iter().filter(|w| crate::level(w) == crate::Level::Error).map(|w| w.key.clone().unwrap()).collect();
        errors.sort();
        assert_eq!(errors, ["feedback.send", "frobnicate", "quality.min", "safety.min", "search.styles.lenny"]);

        let (pf, w) = PolicyFile::parse("[locks\n", None);
        assert!(pf.failed_closed && pf.policy.reports_disabled);
        assert_eq!(pf.policy.max_safety, Some(Safety::Strict));
        assert_eq!(&*w[0].code, codes::POLICY_UNREADABLE);
    }

    #[test]
    fn generic_locks_and_endpoints() {
        let (pf, w) = PolicyFile::parse(
            "[locks]\nsearch.dedupe = \"strong\"\nsafety = \"moderate\"\n[endpoints]\nfeedback.endpoint = \"https://org.example/k\"\n",
            None,
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(pf.policy.safety, Some(Safety::Moderate));
        assert_eq!(pf.locks["search.dedupe"], Value::Str("strong".into()));
        assert_eq!(pf.locks["feedback.endpoint"], Value::Str("https://org.example/k".into()));
    }
}
