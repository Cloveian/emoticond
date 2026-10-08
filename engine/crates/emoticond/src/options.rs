//! Per-query and open-time options (docs/options.md §3 and §6).
//! Kaomoji only; strict never unlocks by query words.
//!
//! Every struct here is `#[non_exhaustive]` with public fields and a
//! `Default` that is the **stranger** default (options.md §9): start from
//! `SearchOptions::default()` (or `Database::defaults()`) and set fields.
//! Serde uses the same snake_case names as the config file and the daemon's
//! `"opts"` object; missing keys take their defaults and unknown keys are
//! ignored.

use crate::error::Warning;
use crate::id::FaceId;
use crate::policy::Policy;
use crate::usage::UsageMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// `NAMES`, `as_str`, `Display` and `FromStr` for an option enum, with the
/// same snake_case names serde uses.
macro_rules! named {
    ($t:ident { $($v:ident => $n:literal),+ $(,)? }) => {
        impl $t {
            /// Every value's name, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$($n),+];
            /// The value's name (as in config files and the daemon's `opts`).
            pub fn as_str(self) -> &'static str {
                match self { $($t::$v => $n),+ }
            }
        }
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl std::str::FromStr for $t {
            type Err = crate::error::ParseOptionError;
            fn from_str(s: &str) -> Result<$t, Self::Err> {
                match s {
                    $($n => Ok($t::$v),)+
                    _ => Err(crate::error::ParseOptionError { value: s.to_string(), expected: $t::NAMES }),
                }
            }
        }
    };
}
pub(crate) use named;

named!(Safety { Strict => "strict", Moderate => "moderate", Off => "off" });
named!(StyleMode { Hide => "hide", Demote => "demote", Allow => "allow" });
named!(Figures { Auto => "auto", Single => "single", Pair => "pair", Any => "any" });
named!(Intensity { Auto => "auto", Low => "low", High => "high" });
named!(Dedupe { Off => "off", Normal => "normal", Strong => "strong" });
named!(UsageWeight { Off => "off", Low => "low", Normal => "normal", High => "high" });
named!(Lang { Auto => "auto", En => "en", Ja => "ja" });
named!(GlyphSearch { Auto => "auto", Off => "off" });
named!(Explain { Off => "off", Reading => "reading", Full => "full" });
named!(Dataset { Auto => "auto", Core => "core", Full => "full" });

/// Largest `limit` a query may ask for.
pub const MAX_LIMIT: u16 = 500;

/// Suggestive content handling (options.md §3.2).
///
/// Ordered from strictest to loosest, so a policy ceiling is a `max`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    /// Faces with suggestive ≥ 1.5 (of 3) are removed; innuendo (0.5–1.5,
    /// the lenny face is about 1) is demoted. Query words never unlock the
    /// removed faces.
    #[default]
    Strict,
    /// Today's behaviour: everything ≥ 0.5 demoted on a curve; a query that
    /// asks (`lewd`, `nsfw`, ...) turns the penalty into a bonus.
    Moderate,
    /// No penalty; asking still gives the bonus.
    Off,
}

/// What to do with one style of face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleMode {
    /// Never shown, even when the query asks for it.
    Hide,
    /// Shown lower (today's penalty).
    Demote,
    /// No penalty.
    Allow,
}

/// Face styles a user may not want (options.md §3.1 `styles.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Styles {
    /// The `( ͡° ͜ʖ ͡°)` family (lenny ≥ 0.5). `Demote` is today's −0.15; a
    /// query that asks (`lenny`) lifts `Demote`/`Allow` to a bonus; `Hide`
    /// beats asking. Default `Demote`.
    pub lenny: StyleMode,
    /// Faces with the crude flag (rude gestures, vulgar text). `Demote` is
    /// today's −0.1 unless a lewd word is in the query. Default `Hide`.
    pub crude: StyleMode,
    /// Faces longer than `SearchOptions::long_at` chars. `Demote` is today's
    /// length penalty; `Hide` drops them. Default `Demote`.
    pub long: StyleMode,
}

impl Default for Styles {
    fn default() -> Styles {
        Styles { lenny: StyleMode::Demote, crude: StyleMode::Hide, long: StyleMode::Demote }
    }
}

impl Styles {
    /// Today's (pre-library) behaviour: everything demoted, nothing hidden.
    pub fn legacy() -> Styles {
        Styles { lenny: StyleMode::Demote, crude: StyleMode::Demote, long: StyleMode::Demote }
    }
}

/// Which way the figures should go (options.md §3.1 `figures`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Figures {
    /// Query words and terms decide; plain emotions lean single (today).
    #[default]
    Auto,
    /// As if the query said `solo`.
    Single,
    /// As if the query said `together`.
    Pair,
    /// No figure preference at all.
    Any,
}

/// Whole-query intensity (options.md §3.1 `intensity`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intensity {
    /// Only the query's own modifiers (`a bit`, `very`).
    #[default]
    Auto,
    /// As if every term were prefixed `a bit`.
    Low,
    /// As if every term were prefixed `very`.
    High,
}

/// Near-duplicate thinning (options.md §3.1 `dedupe`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dedupe {
    /// Keep near-duplicates.
    Off,
    /// Character-set Jaccard 0.55 (today).
    #[default]
    Normal,
    /// Jaccard 0.40.
    Strong,
}

impl Dedupe {
    /// The Jaccard similarity above which a face is a duplicate of one
    /// already listed.
    pub fn threshold(self) -> f32 {
        match self {
            Dedupe::Off => 1.0,
            Dedupe::Normal => 0.55,
            Dedupe::Strong => 0.40,
        }
    }
}

/// How much the caller's `usage` lifts a face.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageWeight {
    Off,
    Low,
    #[default]
    Normal,
    /// Can beat a curated boost, never a pinned canonical face.
    High,
}

/// Query language (options.md §3.1 `lang`). Only `Auto` changes anything
/// today: one index holds English and Japanese keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    #[default]
    Auto,
    En,
    Ja,
}

/// Pasted-glyph substring search (options.md §3.1 `glyph_search`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphSearch {
    /// Text with symbols in it (`ツ`, `¯\_`) finds faces containing it.
    #[default]
    Auto,
    /// Always parse as words.
    Off,
}

/// How much the result explains itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Explain {
    #[default]
    Off,
    /// The one-line reading (`very angry (angry) · pair · pinned faces first`).
    Reading,
    /// Also the parsed terms (`Reading::debug`) and a per-hit score
    /// breakdown (`Hit::why`). Unstable format.
    Full,
}

/// Emotion filters and targets (options.md §3.1 `emotions.*`), keyed by
/// emotion name as the data set lists them (`Database::info().emotions`).
/// Values are normalised intensities in 0..=1. Unknown names are ignored
/// with a warning.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct EmotionFilter {
    /// Faces should be *near* these intensities (`{sad: 0.3}` = mildly sad).
    /// Works with an empty query (browse mode).
    pub target: BTreeMap<String, f32>,
    /// Hard filter: the face's intensity is at least this.
    pub min: BTreeMap<String, f32>,
    /// Hard filter: the face's intensity is at most this.
    pub max: BTreeMap<String, f32>,
}

impl EmotionFilter {
    pub fn is_empty(&self) -> bool {
        self.target.is_empty() && self.min.is_empty() && self.max.is_empty()
    }
}

/// Unstable ranking knobs (options.md §3.4). Only with the
/// `unstable-tuning` feature; may change or vanish in any release.
#[cfg(feature = "unstable-tuning")]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Tuning {
    /// emotion, dense, engine, lexical (default 0.5, 1.2, 1.2, 0.2).
    pub weights: Option<[f32; 4]>,
    /// Break score ties (and order glyph matches of equal length) by the
    /// face's position in the export, as the pre-library engine did, instead
    /// of by `FaceId`. Only so the daemon can stay byte-identical with the
    /// golden snapshot until the id order is blessed; will go away.
    pub legacy_ties: bool,
}

/// Everything a single query can ask for (options.md §3.1, kaomoji only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SearchOptions {
    /// Results to return, after dedupe and filters (1..=500, default 40).
    pub limit: u16,
    /// Results to skip first (paging). Stable only with the same query,
    /// options and seed.
    pub offset: u32,
    pub safety: Safety,
    pub styles: Styles,
    /// The length (chars) above which `styles.long` applies (default 14).
    pub long_at: u16,
    /// Hard cap on length in chars; beats `styles.long`.
    pub max_len: Option<u16>,
    /// Drop entries the model says aren't faces (face score < 0.5).
    pub faces_only: bool,
    pub figures: Figures,
    pub intensity: Intensity,
    pub emotions: EmotionFilter,
    /// Hard cut on predicted quality (1..=8).
    pub min_quality: Option<f32>,
    pub dedupe: Dedupe,
    /// Canonical faces lead single-concept queries (`idk`, `shrug`).
    pub pinned: bool,
    /// Caller-supplied, already-decayed personal popularity.
    pub usage: UsageMap,
    pub usage_weight: UsageWeight,
    /// Seeded jitter on non-pinned scores, 0..=1. 0 is fully deterministic.
    pub variety: f32,
    /// Seed for `variety`; the front-end picks it (per session, per day).
    pub seed: u64,
    /// Never return these faces for this query.
    pub exclude: BTreeSet<FaceId>,
    pub lang: Lang,
    /// Stem or spell-correct a lone unknown word (`shruging` → `shrug`).
    pub correct: bool,
    /// Blend a partial single word's likeliest completions (`embar`).
    pub complete_partial: bool,
    pub glyph_search: GlyphSearch,
    pub explain: Explain,
    /// Unstable ranking knobs (feature `unstable-tuning`).
    #[cfg(feature = "unstable-tuning")]
    pub tuning: Option<Tuning>,
}

impl Default for SearchOptions {
    fn default() -> SearchOptions {
        SearchOptions {
            limit: 40,
            offset: 0,
            safety: Safety::Strict,
            styles: Styles::default(),
            long_at: 14,
            max_len: None,
            faces_only: false,
            figures: Figures::Auto,
            intensity: Intensity::Auto,
            emotions: EmotionFilter::default(),
            min_quality: None,
            dedupe: Dedupe::Normal,
            pinned: true,
            usage: UsageMap::default(),
            usage_weight: UsageWeight::Normal,
            variety: 0.0,
            seed: 0,
            exclude: BTreeSet::new(),
            lang: Lang::Auto,
            correct: true,
            complete_partial: true,
            glyph_search: GlyphSearch::Auto,
            explain: Explain::Off,
            #[cfg(feature = "unstable-tuning")]
            tuning: None,
        }
    }
}

impl SearchOptions {
    /// Today's (pre-library) daemon behaviour: `moderate` safety and every
    /// style demoted, with the given limit. Ranking with these options
    /// reproduces the old engine's scores exactly.
    pub fn legacy(limit: u16) -> SearchOptions {
        SearchOptions { limit, safety: Safety::Moderate, styles: Styles::legacy(), ..SearchOptions::default() }
    }

    /// Clamp out-of-range values in place (options.md §10.1), returning a
    /// warning per key changed. Emotion names are checked by the database,
    /// which knows them.
    pub fn validate(&mut self) -> Vec<Warning> {
        let mut w = Vec::new();
        if self.limit == 0 || self.limit > MAX_LIMIT {
            let to = self.limit.clamp(1, MAX_LIMIT);
            w.push(Warning::new("out_of_range", Some("limit"), format!("limit {} clamped to {to}", self.limit)));
            self.limit = to;
        }
        if self.long_at == 0 {
            w.push(Warning::new("out_of_range", Some("long_at"), "long_at 0 clamped to 1"));
            self.long_at = 1;
        }
        if let Some(q) = self.min_quality {
            let to = if q.is_nan() { 1.0 } else { q.clamp(1.0, 8.0) };
            if to != q {
                w.push(Warning::new("out_of_range", Some("min_quality"), format!("min_quality {q} clamped to {to}")));
                self.min_quality = Some(to);
            }
        }
        if !(0.0..=1.0).contains(&self.variety) {
            let to = if self.variety.is_nan() { 0.0 } else { self.variety.clamp(0.0, 1.0) };
            w.push(Warning::new("out_of_range", Some("variety"), format!("variety {} clamped to {to}", self.variety)));
            self.variety = to;
        }
        for (key, map) in [
            ("emotions.target", &mut self.emotions.target),
            ("emotions.min", &mut self.emotions.min),
            ("emotions.max", &mut self.emotions.max),
        ] {
            for (name, v) in map.iter_mut() {
                if !(0.0..=1.0).contains(v) {
                    let to = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
                    w.push(Warning::new("out_of_range", Some(key), format!("{key}.{name} {v} clamped to {to}")));
                    *v = to;
                }
            }
        }
        w
    }
}

/// Which data set to open (options.md §6): which of `core.kmj` and
/// `full.kmj` a data dir is searched for first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    /// `full` if installed, else `core`.
    Auto,
    /// The default install.
    #[default]
    Core,
    Full,
}

/// Where an overlay layer comes from (options.md §6.1; formats in
/// [`crate::overlay`]). Each source is one layer; layers apply in order,
/// the later winning on conflicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverlaySource {
    /// A directory holding any of `canonical.jsonl`, `phrases.jsonl`,
    /// `boosts.jsonl` and `blocklist.txt`, or one such file (its kind from
    /// its name). Read with the `fs` feature; without it, a warning.
    Path(PathBuf),
    /// One overlay file's contents, with the file name that says what it is
    /// (`canonical.jsonl`, `phrases.jsonl`, `boosts.jsonl`, `blocklist.txt`).
    /// Parsed with the `fs` feature; without it, a warning.
    Inline { name: String, text: String },
    /// A layer built in code (works without `fs`, e.g. in WASM).
    Overlay(crate::overlay::Overlay),
}

impl OverlaySource {
    /// The files whose changes should trigger `Database::reload_overlays`:
    /// for a directory, each of [`OVERLAY_FILES`](crate::overlay::OVERLAY_FILES)
    /// in it (present or not: a file can appear), for a file, itself; none
    /// for inline sources. Hand these to `emoticond_state::Shared::open`.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        match self {
            OverlaySource::Path(p) if p.is_dir() || p.extension().is_none() => {
                crate::overlay::OVERLAY_FILES.iter().map(|(n, _)| p.join(n)).collect()
            }
            OverlaySource::Path(p) => vec![p.clone()],
            _ => Vec::new(),
        }
    }
}

/// How to open a database (options.md §6).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct OpenOptions {
    /// A data file to open (`…/full.kmj`). When set, `data_dirs` and
    /// `dataset` are not consulted.
    pub file: Option<PathBuf>,
    /// Directories to look in, in order; the first holding a data file
    /// (`core.kmj` / `full.kmj`, see `dataset`) wins. The config crate
    /// computes the XDG default; the library takes the list as given.
    pub data_dirs: Vec<PathBuf>,
    pub dataset: Dataset,
    /// User overlays, applied in order after the shipped data (the later
    /// wins). `emoticond-config` lists the state dir, then the user's config
    /// dir, then `data.overlays`. Change them later with
    /// `Database::reload_overlays`.
    pub overlays: Vec<OverlaySource>,
    /// Faces never returned, whatever the options. Change it later with
    /// `Database::set_blocklist`.
    pub blocklist: BTreeSet<FaceId>,
    /// Ceilings and locks applied to every query.
    pub policy: Policy,
    /// What `Database::defaults()` hands out: the front-end's configured
    /// options, the base every query should start from. Change it later
    /// with `Database::set_defaults`.
    pub defaults: SearchOptions,
}

impl OpenOptions {
    /// Open from one data dir with everything else default.
    pub fn new(data_dir: impl Into<PathBuf>) -> OpenOptions {
        OpenOptions { data_dirs: vec![data_dir.into()], ..OpenOptions::default() }
    }

    /// Open one data file with everything else default.
    pub fn file(path: impl Into<PathBuf>) -> OpenOptions {
        OpenOptions { file: Some(path.into()), ..OpenOptions::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stranger_defaults() {
        let o = SearchOptions::default();
        assert_eq!(o.limit, 40);
        assert_eq!(o.safety, Safety::Strict);
        assert_eq!(o.styles.lenny, StyleMode::Demote);
        assert_eq!(o.styles.crude, StyleMode::Hide);
        assert_eq!(o.styles.long, StyleMode::Demote);
        assert_eq!(o.long_at, 14);
        assert!(o.pinned && o.correct && o.complete_partial);
        assert_eq!(o.dedupe.threshold(), 0.55);
        assert_eq!(o.usage_weight, UsageWeight::Normal);
        assert_eq!(o.variety, 0.0);
        assert_eq!(o.explain, Explain::Off);
    }

    #[test]
    fn safety_orders_strict_to_off() {
        assert!(Safety::Strict < Safety::Moderate && Safety::Moderate < Safety::Off);
    }

    #[test]
    fn serde_names_and_partial_input() {
        let o: SearchOptions =
            serde_json::from_str(r#"{"limit":5,"safety":"moderate","styles":{"lenny":"hide"},"explain":"reading","bogus":1}"#)
                .unwrap();
        assert_eq!(o.limit, 5);
        assert_eq!(o.safety, Safety::Moderate);
        assert_eq!(o.styles.lenny, StyleMode::Hide);
        assert_eq!(o.styles.crude, StyleMode::Hide, "unset style keeps its default");
        assert_eq!(o.explain, Explain::Reading);
        let back: SearchOptions = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
        assert!(serde_json::from_str::<SearchOptions>(r#"{"safety":"spicy"}"#).is_err());
    }

    #[test]
    fn validate_clamps_with_warnings() {
        let mut o = SearchOptions { limit: 0, variety: 3.0, min_quality: Some(11.0), ..SearchOptions::default() };
        o.emotions.target.insert("sad".into(), -1.0);
        let w = o.validate();
        assert_eq!(o.limit, 1);
        assert_eq!(o.variety, 1.0);
        assert_eq!(o.min_quality, Some(8.0));
        assert_eq!(o.emotions.target["sad"], 0.0);
        let keys: Vec<_> = w.iter().filter_map(|w| w.key.as_deref()).collect();
        assert_eq!(keys, ["limit", "min_quality", "variety", "emotions.target"]);
        let mut ok = SearchOptions::default();
        assert!(ok.validate().is_empty());
        let mut big = SearchOptions { limit: 9999, ..SearchOptions::default() };
        big.validate();
        assert_eq!(big.limit, MAX_LIMIT);
    }

    #[test]
    fn enum_names_match_serde() {
        fn check<T: std::str::FromStr + std::fmt::Display + Serialize + PartialEq + std::fmt::Debug>(names: &[&str])
        where
            T::Err: std::fmt::Debug,
        {
            for n in names {
                let v: T = n.parse().unwrap();
                assert_eq!(v.to_string(), *n);
                assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{n}\""));
            }
            assert!("nope".parse::<T>().is_err());
        }
        check::<Safety>(Safety::NAMES);
        check::<StyleMode>(StyleMode::NAMES);
        check::<Figures>(Figures::NAMES);
        check::<Intensity>(Intensity::NAMES);
        check::<Dedupe>(Dedupe::NAMES);
        check::<UsageWeight>(UsageWeight::NAMES);
        check::<Lang>(Lang::NAMES);
        check::<GlyphSearch>(GlyphSearch::NAMES);
        check::<Explain>(Explain::NAMES);
        check::<Dataset>(Dataset::NAMES);
        check::<crate::policy::PopularityMode>(crate::policy::PopularityMode::NAMES);
        let e = "spicy".parse::<Safety>().unwrap_err();
        assert_eq!(e.to_string(), "unknown value \"spicy\" (expected one of: strict, moderate, off)");
        assert_eq!(Dataset::default(), Dataset::Core);
    }

    #[test]
    fn legacy_is_moderate_and_demotes() {
        let o = SearchOptions::legacy(30);
        assert_eq!(o.limit, 30);
        assert_eq!(o.safety, Safety::Moderate);
        assert_eq!(o.styles, Styles::legacy());
    }
}
