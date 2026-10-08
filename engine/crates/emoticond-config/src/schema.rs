//! The registry of settings: every key `config.toml` understands, its type,
//! where a settings UI should show it (options.md §8), and how it reads and
//! writes the typed [`Config`].

use crate::config::Config;
use crate::value::{Kind, Value};
use serde::{de::DeserializeOwned, Serialize};
use std::path::PathBuf;

/// Where a settings UI shows a setting (options.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Page {
    /// The four basic controls every launcher shows (§8.1).
    Basic,
    /// The collapsed advanced section (§8.2).
    Advanced,
    /// Only in the config file (§8.3).
    File,
}

/// A description of one setting, for settings UIs and `emoticond config`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SettingInfo {
    /// Dotted key as written in `config.toml` (`search.safety`).
    pub key: &'static str,
    /// Accepted values (`strict | moderate | off`, `integer 1..=500`).
    pub accepts: String,
    /// For enum settings, the choices in order.
    pub choices: &'static [&'static str],
    pub page: Page,
    pub help: &'static str,
    /// The stranger default.
    pub default: Value,
}

pub(crate) struct Spec {
    pub key: &'static str,
    pub kind: Kind,
    pub page: Page,
    pub help: &'static str,
}

const STYLE: &[&str] = &["hide", "demote", "allow"];

macro_rules! spec {
    ($key:literal, $kind:expr, $page:ident, $help:literal) => {
        Spec { key: $key, kind: $kind, page: Page::$page, help: $help }
    };
}

use Kind::*;

pub(crate) static SPECS: &[Spec] = &[
    spec!("search.limit", Int { min: 1, max: 500 }, File, "results per query, after dedupe and filters"),
    spec!("search.safety", Enum(&["strict", "moderate", "off"]), Basic, "content filter for suggestive faces; strict never unlocks"),
    spec!("search.styles.lenny", Enum(STYLE), Basic, "the ( ͡° ͜ʖ ͡°) family"),
    spec!("search.styles.crude", Enum(STYLE), Basic, "rude gestures and vulgar text"),
    spec!("search.styles.long", Enum(STYLE), Advanced, "faces longer than long_at"),
    spec!("search.long_at", Int { min: 1, max: 1000 }, Advanced, "length (chars) where styles.long starts"),
    spec!("search.max_len", OptInt { min: 1, max: 1000 }, Advanced, "hard cap on face length in chars"),
    spec!("search.faces_only", Bool, Advanced, "drop entries that aren't faces"),
    spec!("search.figures", Enum(&["auto", "single", "pair", "any"]), Advanced, "one figure, two, or let the query decide"),
    spec!("search.intensity", Enum(&["auto", "low", "high"]), File, "whole-query intensity"),
    spec!("search.emotions.target", EmotionMap, File, "faces near these intensities"),
    spec!("search.emotions.min", EmotionMap, File, "faces at least this intense"),
    spec!("search.emotions.max", EmotionMap, File, "faces at most this intense"),
    spec!("search.min_quality", OptFloat { min: 1.0, max: 8.0 }, Advanced, "hard cut on predicted quality"),
    spec!("search.dedupe", Enum(&["off", "normal", "strong"]), Advanced, "near-duplicate thinning"),
    spec!("search.pinned", Bool, Advanced, "canonical faces lead single-concept queries"),
    spec!("search.variety", Float { min: 0.0, max: 1.0 }, Advanced, "seeded jitter so repeat queries differ"),
    spec!("search.lang", Enum(&["auto", "en", "ja"]), Advanced, "query language"),
    spec!("search.correct", Bool, File, "stem or spell-correct a lone unknown word"),
    spec!("search.complete_partial", Bool, File, "blend completions of a partial word"),
    spec!("search.glyph_search", Enum(&["auto", "off"]), File, "pasted symbols search by substring"),
    spec!("search.explain", Enum(&["off", "reading", "full"]), File, "how much a result explains itself"),
    spec!("popularity.mode", Enum(&["off", "local", "shared"]), Basic, "learn from what I pick"),
    spec!("popularity.half_life_days", Float { min: 1.0, max: 3650.0 }, Advanced, "how fast old picks fade"),
    spec!("popularity.max_entries", Int { min: 1, max: 10_000_000 }, File, "(term, face) pairs kept"),
    spec!("popularity.store", OptPath, File, "the usage file"),
    spec!("popularity.remember_terms", Bool, File, "remember what each pick was for"),
    spec!("popularity.weight", Enum(&["off", "low", "normal", "high"]), Advanced, "how much your picks lift a face"),
    spec!("popularity.share_interval_days", Int { min: 1, max: 365 }, File, "how often shared counts are queued"),
    spec!("popularity.share_endpoint", OptUrl, File, "where shared counts go"),
    spec!("feedback.menus", Bool, File, "show the report menus"),
    spec!("feedback.send", Bool, Advanced, "send reports (off: saved on this computer only)"),
    spec!("feedback.endpoint", OptUrl, File, "report collector"),
    spec!("feedback.queue", OptPath, File, "the report queue file"),
    spec!("feedback.queue_max", Int { min: 1, max: 1_000_000 }, File, "reports kept in the queue"),
    spec!("feedback.queue_max_age_days", Int { min: 1, max: 3650 }, File, "oldest queued report kept"),
    spec!("feedback.apply_locally", Bool, Advanced, "apply a report's local effects"),
    spec!("feedback.custom_max_chars", Int { min: 1, max: 10_000 }, File, "longest free-text note"),
    spec!("ui.show_reading", Bool, Advanced, "show how the query was read"),
    spec!("ui.max_results", OptInt { min: 1, max: 100_000 }, File, "most results a front-end lists"),
    spec!("ui.start_timeout_ms", Int { min: 0, max: 600_000 }, File, "how long to wait for a helper to start"),
    spec!("daemon.idle_exit", Int { min: 0, max: u32::MAX as i64 }, File, "exit after this many idle seconds; 0 = never"),
    spec!("data.dataset", Enum(&["auto", "core", "full"]), File, "which data set to open"),
    spec!("data.dirs", PathList, File, "data dirs searched before the platform ones"),
    spec!("data.overlays", PathList, File, "extra overlay files or dirs"),
    spec!("data.blocklist", PathList, File, "extra blocklist files"),
    spec!("dev.pick_log", OptPath, File, "log every pick to this file (development)"),
    spec!("advanced.tuning.weights", Weights, File, "UNSTABLE ranking weights"),
];

pub(crate) fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// Keys from older drafts of options.md, removed when it went kaomoji only. They are ignored with an `obsolete_key` warning.
/// The canonical key for a name a user typed: accepts the file form
/// (`search.safety`), the per-query form without `search.` (`safety`,
/// `styles.lenny`), kebab-case (`min-quality`) and a few aliases
/// (`usage_weight` → `popularity.weight`, `tuning.weights`).
pub fn canonical_key(name: &str) -> Option<&'static str> {
    let k = name.trim().to_ascii_lowercase().replace('-', "_");
    let k = match k.as_str() {
        "usage_weight" | "search.usage_weight" => "popularity.weight".to_string(),
        "tuning.weights" | "search.tuning.weights" => "advanced.tuning.weights".to_string(),
        _ => k,
    };
    spec(&k).or_else(|| spec(&format!("search.{k}"))).map(|s| s.key)
}

/// Every setting, in file order.
pub fn settings() -> impl Iterator<Item = SettingInfo> {
    let d = Config::default();
    SPECS.iter().map(move |s| SettingInfo {
        key: s.key,
        accepts: s.kind.describe(),
        choices: if let Kind::Enum(c) = s.kind { c } else { &[] },
        page: s.page,
        help: s.help,
        default: get(&d, s.key),
    })
}

/// Information about one setting (any accepted spelling of the key).
pub fn setting(key: &str) -> Option<SettingInfo> {
    let k = canonical_key(key)?;
    settings().find(|s| s.key == k)
}

fn ename<T: Serialize>(t: &T) -> Value {
    match serde_json::to_value(t) {
        Ok(serde_json::Value::String(s)) => Value::Str(s),
        _ => Value::Unset,
    }
}

fn eparse<T: DeserializeOwned>(v: &Value) -> Option<T> {
    match v {
        Value::Str(s) => serde_json::from_value(serde_json::Value::String(s.clone())).ok(),
        _ => None,
    }
}

fn opt_path(p: &Option<PathBuf>) -> Value {
    p.clone().map(Value::Path).unwrap_or(Value::Unset)
}

fn opt_str(p: &Option<String>) -> Value {
    p.clone().map(Value::Str).unwrap_or(Value::Unset)
}

/// Read a setting from a typed config.
pub(crate) fn get(c: &Config, key: &str) -> Value {
    let s = &c.search;
    match key {
        "search.limit" => Value::Int(s.limit.into()),
        "search.safety" => ename(&s.safety),
        "search.styles.lenny" => ename(&s.styles.lenny),
        "search.styles.crude" => ename(&s.styles.crude),
        "search.styles.long" => ename(&s.styles.long),
        "search.long_at" => Value::Int(s.long_at.into()),
        "search.max_len" => s.max_len.map(|v| Value::Int(v.into())).unwrap_or(Value::Unset),
        "search.faces_only" => Value::Bool(s.faces_only),
        "search.figures" => ename(&s.figures),
        "search.intensity" => ename(&s.intensity),
        "search.emotions.target" => Value::Map(s.emotions.target.iter().map(|(k, v)| (k.clone(), f64::from(*v))).collect()),
        "search.emotions.min" => Value::Map(s.emotions.min.iter().map(|(k, v)| (k.clone(), f64::from(*v))).collect()),
        "search.emotions.max" => Value::Map(s.emotions.max.iter().map(|(k, v)| (k.clone(), f64::from(*v))).collect()),
        "search.min_quality" => s.min_quality.map(|v| Value::Float(f64::from(v))).unwrap_or(Value::Unset),
        "search.dedupe" => ename(&s.dedupe),
        "search.pinned" => Value::Bool(s.pinned),
        "search.variety" => Value::Float(f64::from(s.variety)),
        "search.lang" => ename(&s.lang),
        "search.correct" => Value::Bool(s.correct),
        "search.complete_partial" => Value::Bool(s.complete_partial),
        "search.glyph_search" => ename(&s.glyph_search),
        "search.explain" => ename(&s.explain),
        "popularity.mode" => ename(&c.popularity.mode),
        "popularity.half_life_days" => Value::Float(f64::from(c.popularity.half_life_days)),
        "popularity.max_entries" => Value::Int(c.popularity.max_entries.into()),
        "popularity.store" => opt_path(&c.popularity.store),
        "popularity.remember_terms" => Value::Bool(c.popularity.remember_terms),
        "popularity.weight" => ename(&c.popularity.weight),
        "popularity.share_interval_days" => Value::Int(c.popularity.share_interval_days.into()),
        "popularity.share_endpoint" => opt_str(&c.popularity.share_endpoint),
        "feedback.menus" => Value::Bool(c.feedback.menus),
        "feedback.send" => Value::Bool(c.feedback.send),
        "feedback.endpoint" => opt_str(&c.feedback.endpoint),
        "feedback.queue" => opt_path(&c.feedback.queue),
        "feedback.queue_max" => Value::Int(c.feedback.queue_max.into()),
        "feedback.queue_max_age_days" => Value::Int(c.feedback.queue_max_age_days.into()),
        "feedback.apply_locally" => Value::Bool(c.feedback.apply_locally),
        "feedback.custom_max_chars" => Value::Int(c.feedback.custom_max_chars.into()),
        "ui.show_reading" => Value::Bool(c.ui.show_reading),
        "ui.max_results" => c.ui.max_results.map(|v| Value::Int(v.into())).unwrap_or(Value::Unset),
        "ui.start_timeout_ms" => Value::Int(c.ui.start_timeout_ms.into()),
        "daemon.idle_exit" => Value::Int(c.daemon.idle_exit.into()),
        "data.dataset" => ename(&c.data.dataset),
        "data.dirs" => Value::Paths(c.data.dirs.clone()),
        "data.overlays" => Value::Paths(c.data.overlays.clone()),
        "data.blocklist" => Value::Paths(c.data.blocklist.clone()),
        "dev.pick_log" => opt_path(&c.dev.pick_log),
        "advanced.tuning.weights" => c
            .tuning_weights
            .map(|w| Value::Weights([f64::from(w[0]), f64::from(w[1]), f64::from(w[2]), f64::from(w[3])]))
            .unwrap_or(Value::Unset),
        _ => Value::Unset,
    }
}

fn int<T: TryFrom<i64>>(v: &Value) -> Option<T> {
    match v {
        Value::Int(i) => T::try_from(*i).ok(),
        _ => None,
    }
}

fn opt_int<T: TryFrom<i64>>(v: &Value) -> Option<Option<T>> {
    match v {
        Value::Unset => Some(None),
        _ => int(v).map(Some),
    }
}

fn float(v: &Value) -> Option<f32> {
    match v {
        Value::Float(x) => Some(*x as f32),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn path(v: &Value) -> Option<Option<PathBuf>> {
    match v {
        Value::Unset => Some(None),
        Value::Path(p) => Some(Some(p.clone())),
        _ => None,
    }
}

fn paths(v: &Value) -> Option<Vec<PathBuf>> {
    match v {
        Value::Paths(p) => Some(p.clone()),
        _ => None,
    }
}

fn string(v: &Value) -> Option<Option<String>> {
    match v {
        Value::Unset => Some(None),
        Value::Str(s) => Some(Some(s.clone())),
        _ => None,
    }
}

fn map(v: &Value) -> Option<std::collections::BTreeMap<String, f32>> {
    match v {
        Value::Map(m) => Some(m.iter().map(|(k, v)| (k.clone(), *v as f32)).collect()),
        _ => None,
    }
}

/// Write a setting into a typed config. Values of the wrong shape are
/// ignored (the parsers in `value` never produce them).
pub(crate) fn set(c: &mut Config, key: &str, v: &Value) {
    macro_rules! put {
        ($place:expr, $conv:expr) => {
            if let Some(x) = $conv {
                $place = x;
            }
        };
    }
    let s = &mut c.search;
    match key {
        "search.limit" => put!(s.limit, int(v)),
        "search.safety" => put!(s.safety, eparse(v)),
        "search.styles.lenny" => put!(s.styles.lenny, eparse(v)),
        "search.styles.crude" => put!(s.styles.crude, eparse(v)),
        "search.styles.long" => put!(s.styles.long, eparse(v)),
        "search.long_at" => put!(s.long_at, int(v)),
        "search.max_len" => put!(s.max_len, opt_int(v)),
        "search.faces_only" => put!(s.faces_only, boolean(v)),
        "search.figures" => put!(s.figures, eparse(v)),
        "search.intensity" => put!(s.intensity, eparse(v)),
        "search.emotions.target" => put!(s.emotions.target, map(v)),
        "search.emotions.min" => put!(s.emotions.min, map(v)),
        "search.emotions.max" => put!(s.emotions.max, map(v)),
        "search.min_quality" => {
            if let Value::Unset = v {
                s.min_quality = None
            } else {
                put!(s.min_quality, float(v).map(Some))
            }
        }
        "search.dedupe" => put!(s.dedupe, eparse(v)),
        "search.pinned" => put!(s.pinned, boolean(v)),
        "search.variety" => put!(s.variety, float(v)),
        "search.lang" => put!(s.lang, eparse(v)),
        "search.correct" => put!(s.correct, boolean(v)),
        "search.complete_partial" => put!(s.complete_partial, boolean(v)),
        "search.glyph_search" => put!(s.glyph_search, eparse(v)),
        "search.explain" => put!(s.explain, eparse(v)),
        "popularity.mode" => put!(c.popularity.mode, eparse(v)),
        "popularity.half_life_days" => put!(c.popularity.half_life_days, float(v)),
        "popularity.max_entries" => put!(c.popularity.max_entries, int(v)),
        "popularity.store" => put!(c.popularity.store, path(v)),
        "popularity.remember_terms" => put!(c.popularity.remember_terms, boolean(v)),
        "popularity.weight" => put!(c.popularity.weight, eparse(v)),
        "popularity.share_interval_days" => put!(c.popularity.share_interval_days, int(v)),
        "popularity.share_endpoint" => put!(c.popularity.share_endpoint, string(v)),
        "feedback.menus" => put!(c.feedback.menus, boolean(v)),
        "feedback.send" => put!(c.feedback.send, boolean(v)),
        "feedback.endpoint" => put!(c.feedback.endpoint, string(v)),
        "feedback.queue" => put!(c.feedback.queue, path(v)),
        "feedback.queue_max" => put!(c.feedback.queue_max, int(v)),
        "feedback.queue_max_age_days" => put!(c.feedback.queue_max_age_days, int(v)),
        "feedback.apply_locally" => put!(c.feedback.apply_locally, boolean(v)),
        "feedback.custom_max_chars" => put!(c.feedback.custom_max_chars, int(v)),
        "ui.show_reading" => put!(c.ui.show_reading, boolean(v)),
        "ui.max_results" => put!(c.ui.max_results, opt_int(v)),
        "ui.start_timeout_ms" => put!(c.ui.start_timeout_ms, int(v)),
        "daemon.idle_exit" => put!(c.daemon.idle_exit, int(v)),
        "data.dataset" => put!(c.data.dataset, eparse(v)),
        "data.dirs" => put!(c.data.dirs, paths(v)),
        "data.overlays" => put!(c.data.overlays, paths(v)),
        "data.blocklist" => put!(c.data.blocklist, paths(v)),
        "dev.pick_log" => put!(c.dev.pick_log, path(v)),
        "advanced.tuning.weights" => match v {
            Value::Unset => c.tuning_weights = None,
            Value::Weights(w) => c.tuning_weights = Some([w[0] as f32, w[1] as f32, w[2] as f32, w[3] as f32]),
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_round_trips_through_get_and_set() {
        let d = Config::default();
        for s in SPECS {
            let v = get(&d, s.key);
            let mut c = Config::default();
            set(&mut c, s.key, &v);
            assert_eq!(c, d, "{}", s.key);
            if let Kind::Enum(names) = s.kind {
                for n in names {
                    let mut c = Config::default();
                    set(&mut c, s.key, &Value::Str((*n).into()));
                    assert_eq!(get(&c, s.key), Value::Str((*n).into()), "{} = {n}", s.key);
                }
            }
        }
    }

    #[test]
    fn key_spellings() {
        assert_eq!(canonical_key("safety"), Some("search.safety"));
        assert_eq!(canonical_key("styles.lenny"), Some("search.styles.lenny"));
        assert_eq!(canonical_key("min-quality"), Some("search.min_quality"));
        assert_eq!(canonical_key("usage_weight"), Some("popularity.weight"));
        assert_eq!(canonical_key("feedback.send"), Some("feedback.send"));
        assert_eq!(canonical_key("bogus"), None);
        assert_eq!(setting("safety").unwrap().choices, ["strict", "moderate", "off"]);
    }
}
