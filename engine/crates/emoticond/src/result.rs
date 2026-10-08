//! What searches and lookups return (docs/api-frontends.md §1.2, kaomoji
//! only). All owned values: a result outlives the query and can be sent
//! between threads or serialised as is.

use crate::error::Warning;
use crate::id::FaceId;
use crate::options::{Safety, Styles};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;

/// Badges for a face, so a UI can mark one without a `get()`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Flags(pub u16);

impl Flags {
    /// Suggestive ≥ 0.5 (innuendo or more).
    pub const SUGGESTIVE: Flags = Flags(1 << 0);
    /// Suggestive ≥ 1.5 (what `strict` hides).
    pub const EXPLICIT: Flags = Flags(1 << 1);
    /// The lenny family (lenny ≥ 0.5).
    pub const LENNY: Flags = Flags(1 << 2);
    /// The crude flag.
    pub const CRUDE: Flags = Flags(1 << 3);
    /// More than one figure (a pair or group).
    pub const MULTI: Flags = Flags(1 << 4);
    /// The model says this isn't a face (face < 0.5).
    pub const NOT_FACE: Flags = Flags(1 << 5);
    /// A canonical face pinned for this query (hits only).
    pub const PINNED: Flags = Flags(1 << 6);
    /// Longer than `long_at` chars (14 for entries).
    pub const LONG: Flags = Flags(1 << 7);
    /// Pinned by the user's own overlay (`canonical.jsonl`), not the
    /// shipped data (hits only; always with `PINNED`).
    pub const USER_PIN: Flags = Flags(1 << 8);

    const NAMES: [(Flags, &'static str); 9] = [
        (Flags::SUGGESTIVE, "SUGGESTIVE"),
        (Flags::EXPLICIT, "EXPLICIT"),
        (Flags::LENNY, "LENNY"),
        (Flags::CRUDE, "CRUDE"),
        (Flags::MULTI, "MULTI"),
        (Flags::NOT_FACE, "NOT_FACE"),
        (Flags::PINNED, "PINNED"),
        (Flags::LONG, "LONG"),
        (Flags::USER_PIN, "USER_PIN"),
    ];

    pub const fn empty() -> Flags {
        Flags(0)
    }
    pub const fn bits(self) -> u16 {
        self.0
    }
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }
    pub fn set(&mut self, other: Flags, on: bool) {
        if on {
            self.0 |= other.0
        } else {
            self.0 &= !other.0
        }
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, o: Flags) -> Flags {
        Flags(self.0 | o.0)
    }
}

impl std::ops::BitOrAssign for Flags {
    fn bitor_assign(&mut self, o: Flags) {
        self.0 |= o.0;
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = Flags::NAMES.iter().filter(|(fl, _)| self.contains(*fl)).map(|(_, n)| *n).collect();
        write!(f, "Flags({})", names.join(" | "))
    }
}

/// A face's per-component score (`explain = full`). Unstable: the parts
/// follow the ranking's internals and change with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HitWhy {
    /// Pinned canonical bonus.
    pub canon: f32,
    /// Emotion match (before weighting).
    pub emotion: f32,
    /// e5 neighbour-list credit (before weighting).
    pub dense: f32,
    /// Affect-engine neighbour-list credit (before weighting).
    pub engine: f32,
    /// Tag-word credit (before weighting).
    pub lexical: f32,
    /// Everything else added or subtracted: figures, quality, cute, boost,
    /// intensity, the not-a-face/length/suggestive/lenny/crude terms.
    pub adjust: f32,
    /// Personal popularity.
    pub usage: f32,
    /// `variety` jitter.
    pub jitter: f32,
    /// The final score.
    pub total: f32,
}

/// One result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Hit {
    pub id: FaceId,
    pub text: String,
    /// Higher is better. Only comparable within one result.
    pub score: f32,
    /// 0-based position in the whole ranking (so `offset` + index).
    pub rank: u32,
    pub flags: Flags,
    /// `explain = full` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<HitWhy>,
}

/// Where a result's `term_key` comes from. Shared popularity may only
/// send shipped terms (options.md §4.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermSource {
    /// No term key.
    #[default]
    None,
    /// A vocab term, phrase or situation from the shipped data.
    Shipped,
    /// The user's overlay: a user phrase, or a query only the user pinned
    /// faces for. Never sent with shared popularity.
    Overlay,
}

/// A query's results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchResult {
    /// Best first; ties broken by ascending `FaceId`.
    pub hits: Vec<Hit>,
    /// The word a lone unknown word was read as (`shruging` → `shrug`).
    pub corrected: Option<String>,
    /// `explain = reading | full`.
    pub reading: Option<Reading>,
    /// The parsed concept: keys usage (`Pick`) and reports.
    pub term_key: Option<String>,
    /// Where `term_key` comes from.
    #[serde(default)]
    pub term_source: TermSource,
    /// Options the policy changed.
    pub clamped: Vec<Cow<'static, str>>,
    /// Options that were out of range, unknown emotion names, and so on.
    pub warnings: Vec<Warning>,
    /// The safety level in force (after policy).
    pub safety: Safety,
    /// The styles in force (after policy).
    pub styles: Styles,
}

/// How the query was read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ReadMode {
    /// Nothing typed.
    #[default]
    Empty,
    /// One plain concept (`idk`, `happy`).
    Concept,
    /// Several terms (`shy proud`, `a bit sad`).
    Terms,
    /// A situation from the hand-written list (`movie night`).
    Situation,
    /// A longer sentence: terms are averaged.
    Sentence,
    /// A partial word, read as its likeliest completions.
    Partial { completions: Vec<String> },
    /// Pasted symbols: faces containing them.
    Glyph,
    /// No terms, only tag words.
    Tags,
    /// Nothing recognised.
    Nothing,
}

/// `a bit` / `very`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Low,
    High,
}

/// One parsed term, for UIs that render chips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReadTerm {
    /// The vocab term, phrase key or situation words.
    pub term: String,
    /// Its strongest emotions (up to two).
    pub emotions: Vec<String>,
    pub modifier: Option<Modifier>,
    pub negated: bool,
    /// A phrase from the user's overlay (the reading line says `yours`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user: bool,
}

/// The reading line and its parts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reading {
    /// `very angry (angry) + shy (shy, scared) · pair · pinned faces first`
    pub line: String,
    pub mode: ReadMode,
    pub terms: Vec<ReadTerm>,
    /// `explain = full`: the parser's dump (today's `--explain`). Unstable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<String>,
}

/// Graded attributes of a face, from the models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Attrs {
    /// Share of the face that is more than one figure, 0..=1.
    pub multi: f32,
    pub cute: f32,
    pub intensity: f32,
    /// 0..=3: about 1 innuendo, 2 sexual, 3 explicit.
    pub suggestive: f32,
    pub lenny: f32,
    /// How much it is a face at all, 0..=1.
    pub face: f32,
}

/// Everything known about one face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entry {
    pub id: FaceId,
    pub text: String,
    /// Predicted quality, 1..=8.
    pub quality: f32,
    pub flags: Flags,
    pub attrs: Attrs,
    /// Normalised emotion intensities, in `DbInfo::emotions` order.
    pub emotions: Vec<(String, f32)>,
    /// Concepts it is a pinned canonical face for.
    pub canonical_for: Vec<String>,
}

impl Entry {
    /// The `n` strongest emotions, strongest first.
    pub fn top_emotions(&self, n: usize) -> Vec<(String, f32)> {
        let mut v = self.emotions.clone();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }
}

/// Where a completion comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompletionSource {
    /// A vocabulary term (emotion word, lexicon key or tag).
    Term,
    /// A spelling from the user's overlay phrases.
    Overlay,
}

/// A typeahead suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Completion {
    pub text: String,
    pub source: CompletionSource,
    /// Has pinned canonical faces (shipped or the user's).
    pub pinned: bool,
}

/// What `Database::info()` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DbInfo {
    /// This library's version.
    pub engine_version: String,
    /// The data file's format version (`"3.0"`).
    pub format: String,
    /// The dir of the data file opened ("" when opened from bytes).
    pub data_dir: String,
    /// The data file opened ("" when opened from bytes).
    pub path: String,
    /// The data release (`2026.10.0`, or `dev`).
    pub data_version: String,
    /// The data set: `full`, `core` or `lite`.
    pub dataset: String,
    /// SPDX id of the data licence (the text: `Database::licence()`).
    pub licence: String,
    /// SHA-256 of the file's contents after the header, in hex: names
    /// exactly this data (reports' `EngineStamp`).
    pub content_hash: String,
    /// The file's size.
    pub bytes: u64,
    pub faces: usize,
    pub terms: usize,
    pub situations: usize,
    pub phrases: usize,
    /// Concepts with pinned canonical faces.
    pub canonical: usize,
    /// Terms with curated boosts.
    pub boosts: usize,
    /// Emotion names, in the data's order.
    pub emotions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_ops() {
        let mut f = Flags::LENNY | Flags::PINNED;
        assert!(f.contains(Flags::LENNY) && !f.contains(Flags::CRUDE));
        f.set(Flags::LENNY, false);
        assert_eq!(f, Flags::PINNED);
        f |= Flags::CRUDE;
        assert_eq!(format!("{f:?}"), "Flags(CRUDE | PINNED)");
        assert_eq!(serde_json::to_string(&f).unwrap(), (Flags::CRUDE.0 | Flags::PINNED.0).to_string());
        assert!(Flags::empty().is_empty());
    }

    #[test]
    fn top_emotions_sorts_and_breaks_ties_by_name() {
        let e = Entry {
            id: FaceId::of_text("x"),
            text: "x".into(),
            quality: 5.0,
            flags: Flags::empty(),
            attrs: Attrs::default(),
            emotions: vec![("sad".into(), 0.2), ("happy".into(), 0.5), ("angry".into(), 0.2)],
            canonical_for: vec![],
        };
        let t = e.top_emotions(2);
        assert_eq!(t, vec![("happy".to_string(), 0.5), ("angry".to_string(), 0.2)]);
    }

    #[test]
    fn reading_serialises_mode_with_a_tag() {
        let r = Reading { line: "completing: sad".into(), mode: ReadMode::Partial { completions: vec!["sad".into()] }, ..Reading::default() };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains(r#""mode":{"kind":"partial","completions":["sad"]}"#), "{j}");
        assert_eq!(serde_json::from_str::<Reading>(&j).unwrap(), r);
    }
}
