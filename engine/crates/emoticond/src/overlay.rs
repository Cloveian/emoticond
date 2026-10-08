//! User overlays (docs/options.md §6.1):
//! the user's own canonical pins, search phrases, boosts and hidden faces,
//! layered over the shipped data without recompiling it.
//!
//! An [`Overlay`] is one layer of typed tables. It can be built in code
//! (WASM, tests, front-ends that keep their own store) or, with the `fs`
//! feature, parsed from the same JSONL formats as the shipped files:
//!
//! | File | Rows |
//! |---|---|
//! | `canonical.jsonl` | `{"term","text"\|"id","rank"}` pins a face for a term; `{"term","clear":true}` drops the picks below this layer |
//! | `phrases.jsonl` | `{"key","match":[...],"p":{...},"level","pair","words","attr"}`, or `{"key","match":[...],"alias_of":"shrug"}` |
//! | `boosts.jsonl` | `{"term","id"\|"text","boost":-3..3}`; the id in either form (`k2026da3e4989` or `2026da3e4989`) |
//! | `blocklist.txt` | one face id (either form) or exact face text per line; `# ` comments |
//!
//! A directory holds any of those four files ([`OVERLAY_FILES`]). Layers
//! apply in order after the shipped data; on a conflict the later layer
//! wins. `emoticond-config` lists the machine-written state dir first, then
//! the hand-written config dir, then `data.overlays`, so the user's own
//! files beat what feedback wrote. Bad lines and rows naming faces the data
//! set does not have are warnings, never errors: a stale overlay must not
//! stop a picker from starting.
//!
//! How each table applies:
//! - **pins**: per term, a layer's pins (by `rank`, then file order) go
//!   before everything below it: earlier layers' pins and the shipped picks.
//!   At most three faces lead a term ([`MAX_PINS`]). Usage never beats a
//!   pin (options.md §6.2).
//! - **phrases**: user spellings win over shipped phrases with the same
//!   spelling, and over a vocab word of the same length. They are tokenised
//!   like shipped ones (`emoticond::tokens`). The reading marks them as the
//!   user's, and a query whose concept comes from them reports
//!   `TermSource::Overlay`.
//! - **boosts** (−3..3, `W_BOOST` per unit, as shipped boosts) merge with
//!   the shipped boosts, the overlay winning per (term, face). They apply to
//!   queries whose concept (`term_key`) is the term. A negative boost on a
//!   shipped canonical pick also unpins it for that term ("doesn't fit"
//!   must be able to move the pinned face); it never unpins the user's own
//!   pins.
//! - **hidden**: never returned, like the blocklist.

use crate::data::format::N_EMO;
use crate::emo::{tokens, EmoDb};
use crate::error::{Level, Warning};
use crate::id::FaceId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The most faces that lead one term (the canonical bonus has three ranks).
pub const MAX_PINS: usize = 3;

/// The longest phrase spelling, in tokens (as in the data format).
pub const MAX_PHRASE_TOKENS: usize = 8;

/// What an overlay file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    /// `canonical.jsonl`: pins and clears.
    Canonical,
    /// `phrases.jsonl`: search phrases and aliases.
    Phrases,
    /// `boosts.jsonl`: signed boosts per term and face.
    Boosts,
    /// `blocklist.txt`: hidden faces.
    Blocklist,
}

/// The file names an overlay directory is read for, in the order read.
pub const OVERLAY_FILES: [(&str, OverlayKind); 4] = [
    ("canonical.jsonl", OverlayKind::Canonical),
    ("phrases.jsonl", OverlayKind::Phrases),
    ("boosts.jsonl", OverlayKind::Boosts),
    ("blocklist.txt", OverlayKind::Blocklist),
];

impl OverlayKind {
    /// The kind a file name says: `canonical.jsonl`, `phrases.jsonl`,
    /// `boosts.jsonl`, `blocklist.txt`, or a name starting with
    /// `canonical_`, `phrases_`, `lexicon_phrases`, `boosts_` or
    /// `blocklist_` (so the shipped `canonical_hand.jsonl` is an overlay
    /// as is). `None` for anything else.
    pub fn from_file_name(name: &str) -> Option<OverlayKind> {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        let is = |base: &str| stem == base || stem.strip_prefix(base).is_some_and(|r| r.starts_with('_'));
        if is("canonical") {
            Some(OverlayKind::Canonical)
        } else if is("phrases") || is("lexicon_phrases") {
            Some(OverlayKind::Phrases)
        } else if is("boosts") {
            Some(OverlayKind::Boosts)
        } else if is("blocklist") {
            Some(OverlayKind::Blocklist)
        } else {
            None
        }
    }
}

/// A face pinned for a term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CanonPin {
    /// The concept, as queries are keyed (`shrug`, `table flip`); stored
    /// trimmed and lowercased.
    pub term: String,
    pub id: FaceId,
    /// 1 first. Pins of one layer are ordered by rank, then as given.
    pub rank: u32,
}

impl CanonPin {
    pub fn new(term: &str, id: FaceId, rank: u32) -> CanonPin {
        CanonPin { term: norm_term(term), id, rank }
    }
}

/// A signed boost for one face under one term.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OverlayBoost {
    /// The concept (`term_key`); stored trimmed and lowercased.
    pub term: String,
    pub id: FaceId,
    /// −3..=3; negative demotes ("doesn't fit" writes −3).
    pub boost: f32,
}

impl OverlayBoost {
    pub fn new(term: &str, id: FaceId, boost: f32) -> OverlayBoost {
        OverlayBoost { term: norm_term(term), id, boost }
    }
}

/// A search phrase: a key and its spellings, meaning an emotion profile
/// (with optional tag words), or an alias of a known concept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OverlayPhrase {
    /// The concept this phrase is (also a spelling).
    pub key: String,
    /// More spellings (`"so over it"`, typos).
    pub spellings: Vec<String>,
    /// Read the spellings as this concept instead: a user phrase, a shipped
    /// phrase, or a vocab term (`"alias_of":"shrug"`). The fields below are
    /// then ignored.
    pub alias_of: Option<String>,
    /// Emotion name -> weight (normalised to sum 1 when resolved).
    pub p: BTreeMap<String, f32>,
    /// 1 mild, 2 plain, 3 strong.
    pub level: Option<u8>,
    /// Some(true) wants pairs, Some(false) single faces.
    pub pair: Option<bool>,
    /// Tag words: faces tagged with them (and their e5 neighbours) come.
    pub words: Vec<String>,
    pub cute: bool,
    pub lenny: bool,
    pub suggestive: bool,
}

impl OverlayPhrase {
    /// A phrase meaning this emotion profile (`[("happy", 0.6), ("excited", 0.4)]`).
    pub fn new(key: &str, p: &[(&str, f32)]) -> OverlayPhrase {
        OverlayPhrase { key: key.to_string(), p: p.iter().map(|(k, v)| (k.to_string(), *v)).collect(), ..OverlayPhrase::default() }
    }

    /// A spelling of a known concept.
    pub fn alias(key: &str, of: &str) -> OverlayPhrase {
        OverlayPhrase { key: key.to_string(), alias_of: Some(of.to_string()), ..OverlayPhrase::default() }
    }

    /// Add spellings.
    pub fn spellings<I: IntoIterator<Item = S>, S: Into<String>>(mut self, s: I) -> OverlayPhrase {
        self.spellings.extend(s.into_iter().map(Into::into));
        self
    }

    /// Add tag words.
    pub fn words<I: IntoIterator<Item = S>, S: Into<String>>(mut self, w: I) -> OverlayPhrase {
        self.words.extend(w.into_iter().map(Into::into));
        self
    }
}

/// One overlay layer: everything one directory (or set of files) says.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Overlay {
    pub pins: Vec<CanonPin>,
    /// Terms whose picks below this layer (shipped, earlier layers) are dropped.
    pub clear: BTreeSet<String>,
    pub phrases: Vec<OverlayPhrase>,
    pub boosts: Vec<OverlayBoost>,
    /// Faces never returned.
    pub hidden: BTreeSet<FaceId>,
}

fn norm_term(t: &str) -> String {
    t.trim().to_lowercase()
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay::default()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.clear.is_empty() && self.phrases.is_empty() && self.boosts.is_empty() && self.hidden.is_empty()
    }

    /// Pin `id` for `term` at `rank` (1 first).
    pub fn pin(&mut self, term: &str, id: FaceId, rank: u32) -> &mut Overlay {
        self.pins.push(CanonPin::new(term, id, rank));
        self
    }

    /// Drop the picks below this layer for `term`.
    pub fn clear(&mut self, term: &str) -> &mut Overlay {
        self.clear.insert(norm_term(term));
        self
    }

    /// Boost (or with a negative value, demote) `id` for `term`.
    pub fn boost(&mut self, term: &str, id: FaceId, boost: f32) -> &mut Overlay {
        self.boosts.push(OverlayBoost::new(term, id, boost));
        self
    }

    pub fn phrase(&mut self, p: OverlayPhrase) -> &mut Overlay {
        self.phrases.push(p);
        self
    }

    pub fn hide(&mut self, id: FaceId) -> &mut Overlay {
        self.hidden.insert(id);
        self
    }

    /// Add another layer's rows after this one's (so they win).
    pub fn extend(&mut self, o: Overlay) {
        self.pins.extend(o.pins);
        self.clear.extend(o.clear);
        self.phrases.extend(o.phrases);
        self.boosts.extend(o.boosts);
        self.hidden.extend(o.hidden);
    }
}

// ---- parsing (feature `fs`) -------------------------------------------------

#[cfg(feature = "fs")]
mod parse {
    use super::*;
    use serde_json::Value;

    fn bad(source: &str, line: usize, msg: impl std::fmt::Display) -> Warning {
        Warning::new("overlay_bad_line", Some("overlays"), format!("{source}:{line}: {msg}; line ignored"))
    }

    /// A face named by `id` (either form) or `text`.
    fn face(v: &Value) -> Result<FaceId, String> {
        if let Some(s) = v.get("id").and_then(Value::as_str) {
            return FaceId::parse_any(s).ok_or_else(|| format!("not a face id: {s:?}"));
        }
        if let Some(t) = v.get("text").and_then(Value::as_str) {
            if t.is_empty() {
                return Err("empty \"text\"".into());
            }
            return Ok(FaceId::of_text(t));
        }
        Err("needs \"id\" or \"text\"".into())
    }

    fn term(v: &Value, field: &str) -> Result<String, String> {
        match v.get(field).and_then(Value::as_str).map(norm_term) {
            Some(t) if !t.is_empty() => Ok(t),
            _ => Err(format!("needs a non-empty \"{field}\"")),
        }
    }

    fn strings(v: &Value, field: &str) -> Result<Vec<String>, String> {
        match v.get(field) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().map(String::from).ok_or_else(|| format!("\"{field}\" must be a list of strings")))
                .collect(),
            Some(_) => Err(format!("\"{field}\" must be a list of strings")),
        }
    }

    fn canonical_row(o: &mut Overlay, v: &Value) -> Result<(), String> {
        let t = term(v, "term")?;
        if v.get("clear").and_then(Value::as_bool) == Some(true) {
            o.clear.insert(t);
            return Ok(());
        }
        let id = face(v)?;
        let rank = match v.get("rank") {
            None | Some(Value::Null) => 1,
            Some(r) => r.as_u64().filter(|r| *r >= 1).ok_or("\"rank\" must be a whole number >= 1")?.min(u64::from(u32::MAX)) as u32,
        };
        o.pins.push(CanonPin { term: t, id, rank });
        Ok(())
    }

    fn boost_row(o: &mut Overlay, v: &Value, w: &mut Vec<Warning>, source: &str, line: usize) -> Result<(), String> {
        let t = term(v, "term")?;
        let id = face(v)?;
        let b = match v.get("boost") {
            None | Some(Value::Null) => 1.0,
            Some(b) => b.as_f64().ok_or("\"boost\" must be a number")?,
        };
        if !b.is_finite() {
            return Err("\"boost\" must be finite".into());
        }
        let c = b.clamp(-3.0, 3.0);
        if c != b {
            w.push(Warning::new("overlay_out_of_range", Some("overlays"), format!("{source}:{line}: boost {b} clamped to {c}")));
        }
        o.boosts.push(OverlayBoost { term: t, id, boost: c as f32 });
        Ok(())
    }

    fn phrase_row(o: &mut Overlay, v: &Value) -> Result<(), String> {
        let key = v.get("key").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()).ok_or("needs a non-empty \"key\"")?;
        let mut ph = OverlayPhrase { key: key.to_string(), spellings: strings(v, "match")?, ..OverlayPhrase::default() };
        if let Some(a) = v.get("alias_of") {
            let a = a.as_str().map(str::trim).filter(|a| !a.is_empty()).ok_or("\"alias_of\" must be a non-empty string")?;
            ph.alias_of = Some(a.to_string());
            o.phrases.push(ph);
            return Ok(());
        }
        match v.get("p") {
            Some(Value::Object(m)) => {
                for (k, x) in m {
                    let x = x.as_f64().filter(|x| x.is_finite() && *x >= 0.0).ok_or_else(|| format!("\"p\".{k} must be a number >= 0"))?;
                    ph.p.insert(k.clone(), x as f32);
                }
            }
            None | Some(Value::Null) => {}
            Some(_) => return Err("\"p\" must be an object of emotion -> weight".into()),
        }
        ph.level = match v.get("level") {
            None | Some(Value::Null) => None,
            Some(l) => Some(l.as_u64().filter(|l| (1..=3).contains(l)).ok_or("\"level\" must be 1, 2 or 3")? as u8),
        };
        ph.pair = match v.get("pair") {
            None | Some(Value::Null) => None,
            Some(p) => Some(p.as_bool().ok_or("\"pair\" must be true or false")?),
        };
        ph.words = strings(v, "words")?;
        for a in strings(v, "attr")? {
            match a.as_str() {
                "cute" => ph.cute = true,
                "lenny" => ph.lenny = true,
                "suggestive" => ph.suggestive = true,
                _ => {}
            }
        }
        if ph.p.values().all(|x| *x == 0.0) && ph.words.is_empty() {
            return Err("a phrase needs \"p\", \"words\" or \"alias_of\"".into());
        }
        o.phrases.push(ph);
        Ok(())
    }

    fn blocklist_line(o: &mut Overlay, raw: &[u8]) -> Result<(), String> {
        let s = std::str::from_utf8(raw).map_err(|_| "not UTF-8".to_string())?;
        let l = s.trim();
        if l.is_empty() || l == "#" || l.starts_with("# ") || l.starts_with("#\t") {
            return Ok(());
        }
        if let Some(id) = FaceId::parse_any(l) {
            o.hidden.insert(id);
            return Ok(());
        }
        if l.len() > 1 && l.starts_with('k') && l.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("not a face id: {l:?}"));
        }
        if l.chars().count() > 300 {
            return Err("line longer than 300 characters".into());
        }
        o.hidden.insert(FaceId::of_text(l));
        Ok(())
    }

    impl Overlay {
        /// Parse one overlay file's contents. `source` names it in warnings
        /// (a path, or a label for inline text). Bad lines are skipped with a
        /// warning; nothing here fails.
        pub fn parse(kind: OverlayKind, text: &str, source: &str) -> (Overlay, Vec<Warning>) {
            let mut o = Overlay::default();
            let mut w = Vec::new();
            let text = text.strip_prefix('\u{feff}').unwrap_or(text);
            if kind == OverlayKind::Blocklist {
                for (n, raw) in text.as_bytes().split(|b| *b == b'\n').enumerate() {
                    if let Err(e) = blocklist_line(&mut o, raw) {
                        w.push(bad(source, n + 1, e));
                    }
                }
                return (o, w);
            }
            for (n, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with("//") {
                    continue;
                }
                let v: Value = match serde_json::from_str(line) {
                    Ok(v @ Value::Object(_)) => v,
                    Ok(_) => {
                        w.push(bad(source, n + 1, "not a JSON object"));
                        continue;
                    }
                    Err(e) => {
                        w.push(bad(source, n + 1, e));
                        continue;
                    }
                };
                let r = match kind {
                    OverlayKind::Canonical => canonical_row(&mut o, &v),
                    OverlayKind::Phrases => phrase_row(&mut o, &v),
                    OverlayKind::Boosts => boost_row(&mut o, &v, &mut w, source, n + 1),
                    OverlayKind::Blocklist => unreachable!(),
                };
                if let Err(e) = r {
                    w.push(bad(source, n + 1, e));
                }
            }
            (o, w)
        }

        /// Read an overlay file (its kind from its name, see
        /// [`OverlayKind::from_file_name`]) or a directory holding any of
        /// [`OVERLAY_FILES`]. Files missing from a directory are empty; a
        /// missing path is an `Info` warning (`overlay_missing`), an
        /// unreadable file or an unknown file name a warning.
        pub fn read(path: &std::path::Path) -> (Overlay, Vec<Warning>) {
            let mut o = Overlay::default();
            let mut w = Vec::new();
            if path.is_dir() {
                for (name, kind) in OVERLAY_FILES {
                    let p = path.join(name);
                    if p.is_file() {
                        read_file(&mut o, &mut w, &p, kind);
                    }
                }
                return (o, w);
            }
            if !path.exists() {
                w.push(
                    Warning::new("overlay_missing", Some("overlays"), format!("{}: no such overlay file or directory", path.display()))
                        .with_level(Level::Info),
                );
                return (o, w);
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let Some(kind) = OverlayKind::from_file_name(name) else {
                w.push(Warning::new(
                    "overlay_unknown_file",
                    Some("overlays"),
                    format!("{}: not an overlay file name (canonical.jsonl, phrases.jsonl, boosts.jsonl, blocklist.txt); ignored", path.display()),
                ));
                return (o, w);
            };
            read_file(&mut o, &mut w, path, kind);
            (o, w)
        }
    }

    fn read_file(o: &mut Overlay, w: &mut Vec<Warning>, p: &std::path::Path, kind: OverlayKind) {
        match std::fs::read(p) {
            Ok(b) => {
                let (one, mut ww) = Overlay::parse(kind, &String::from_utf8_lossy(&b), &p.display().to_string());
                o.extend(one);
                w.append(&mut ww);
            }
            Err(e) => w.push(Warning::new("overlay_unreadable", Some("overlays"), format!("{}: {e}; ignored", p.display()))),
        }
    }
}

/// Turn sources into layers, in order (one layer per source).
pub(crate) fn load_sources(sources: &[crate::options::OverlaySource]) -> (Vec<Overlay>, Vec<Warning>) {
    use crate::options::OverlaySource;
    let mut layers = Vec::with_capacity(sources.len());
    let mut w = Vec::new();
    for s in sources {
        match s {
            OverlaySource::Overlay(o) => layers.push(o.clone()),
            #[cfg(feature = "fs")]
            OverlaySource::Path(p) => {
                let (o, mut ww) = Overlay::read(p);
                layers.push(o);
                w.append(&mut ww);
            }
            #[cfg(feature = "fs")]
            OverlaySource::Inline { name, text } => match OverlayKind::from_file_name(name) {
                Some(kind) => {
                    let (o, mut ww) = Overlay::parse(kind, text, name);
                    layers.push(o);
                    w.append(&mut ww);
                }
                None => w.push(Warning::new("overlay_unknown_file", Some("overlays"), format!("{name}: not an overlay file name; ignored"))),
            },
            #[cfg(not(feature = "fs"))]
            OverlaySource::Path(_) | OverlaySource::Inline { .. } => w.push(Warning::new(
                "unsupported",
                Some("overlays"),
                "overlay files need the `fs` feature; pass OverlaySource::Overlay values instead",
            )),
        }
    }
    (layers, w)
}

// ---- resolved against a data set ---------------------------------------------

/// A user phrase resolved against the data: what `push_phrase` reads.
#[derive(Debug, Clone)]
pub(crate) struct UserPhrase {
    /// The concept: the phrase's key, or for an alias the target's key.
    pub key: String,
    pub p: [f32; N_EMO],
    pub m: Option<f32>,
    pub level: Option<u8>,
    pub pair: Option<bool>,
    pub words: Vec<String>,
    pub cute: bool,
    pub lenny: bool,
    pub lewd: bool,
}

/// A term's faces after the overlays: (ordinal, pinned by the user), best first.
#[derive(Debug, Clone, Default)]
pub(crate) struct CanonList {
    pub faces: Vec<(u32, bool)>,
}

impl CanonList {
    pub fn has_user(&self) -> bool {
        self.faces.iter().any(|f| f.1)
    }
}

/// Every overlay layer merged and resolved to ordinals: what a query reads.
/// Empty when no overlays are given, and then every lookup falls through to
/// the shipped tables unchanged.
#[derive(Debug, Clone, Default)]
pub(crate) struct OverlayTables {
    /// Terms whose canonical faces the overlays changed (or added).
    pub canon: HashMap<String, CanonList>,
    pub phrases: Vec<UserPhrase>,
    /// spelling (tokens joined by spaces) -> phrase
    pub spell: HashMap<String, usize>,
    /// the longest user spelling, in tokens (0 = none)
    pub phrase_max: usize,
    /// term -> face -> boost (the later layer's value)
    pub boosts: HashMap<String, HashMap<FaceId, f32>>,
    pub hidden: BTreeSet<FaceId>,
}

impl OverlayTables {
    /// User spellings, sorted (for correction and completion).
    pub fn spellings(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.spell.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }
}

fn unknown_face(what: &str, term: &str, id: FaceId) -> Warning {
    Warning::new("overlay_unknown_face", Some("overlays"), format!("{what} for {term:?}: face {id} is not in this data set; ignored"))
        .with_level(Level::Info)
}

/// Merge layers in order and resolve them against the data.
pub(crate) fn resolve(e: &EmoDb, layers: &[Overlay]) -> (OverlayTables, Vec<Warning>) {
    let st = &e.st;
    let mut t = OverlayTables::default();
    let mut w = Vec::new();

    // boosts: later rows win per (term, id)
    for l in layers {
        for b in &l.boosts {
            if b.term.is_empty() || !b.boost.is_finite() {
                continue;
            }
            if st.by_fid(b.id).is_none() {
                w.push(unknown_face("boost", &b.term, b.id));
            }
            t.boosts.entry(b.term.clone()).or_default().insert(b.id, b.boost.clamp(-3.0, 3.0));
        }
    }

    // pins and clears, layer by layer
    let shipped = |term: &str| -> Vec<(u32, bool)> {
        st.canonical(term).unwrap_or_default().into_iter().map(|(f, _)| (f, false)).collect()
    };
    let mut lists: BTreeMap<String, Vec<(u32, bool)>> = BTreeMap::new();
    for l in layers {
        let mut by_term: BTreeMap<&str, Vec<(u32, usize, u32)>> = BTreeMap::new();
        for (k, p) in l.pins.iter().enumerate() {
            if p.term.is_empty() {
                continue;
            }
            match st.by_fid(p.id) {
                Some(o) => by_term.entry(p.term.as_str()).or_default().push((p.rank, k, o)),
                None => w.push(unknown_face("pin", &p.term, p.id)),
            }
        }
        let terms: BTreeSet<&str> = by_term.keys().copied().chain(l.clear.iter().map(String::as_str)).collect();
        for term in terms {
            let cur = lists.entry(term.to_string()).or_insert_with(|| shipped(term));
            if l.clear.contains(term) {
                cur.clear();
            }
            let mut mine = by_term.remove(term).unwrap_or_default();
            mine.sort();
            let mut next: Vec<(u32, bool)> = mine.into_iter().map(|(_, _, o)| (o, true)).collect();
            next.append(cur);
            *cur = next;
        }
    }
    // a demote unpins a shipped pick for that term
    for (term, m) in &t.boosts {
        let demoted: HashSet<FaceId> = m.iter().filter(|(_, b)| **b < 0.0).map(|(id, _)| *id).collect();
        if demoted.is_empty() {
            continue;
        }
        let hits_shipped = st.canonical(term).is_some_and(|v| v.iter().any(|(f, _)| demoted.contains(&st.fid(*f as usize))));
        if !lists.contains_key(term) && !hits_shipped {
            continue;
        }
        let cur = lists.entry(term.clone()).or_insert_with(|| shipped(term));
        cur.retain(|&(f, user)| user || !demoted.contains(&st.fid(f as usize)));
    }
    for (term, mut v) in lists {
        let mut seen = HashSet::new();
        v.retain(|f| seen.insert(f.0));
        v.truncate(MAX_PINS);
        t.canon.insert(term, CanonList { faces: v });
    }

    // phrases: resolve each layer's rows in order; later spellings win
    for l in layers {
        for ph in &l.phrases {
            let key = norm_term(&ph.key);
            let Some(up) = resolve_phrase(e, &t, ph, &key, &mut w) else { continue };
            let i = t.phrases.len();
            t.phrases.push(up);
            for s in std::iter::once(&ph.key).chain(&ph.spellings) {
                let toks = tokens(s);
                if toks.is_empty() {
                    continue;
                }
                if toks.len() > MAX_PHRASE_TOKENS {
                    w.push(Warning::new(
                        "overlay_bad_line",
                        Some("overlays"),
                        format!("phrase {key:?}: spelling {s:?} is longer than {MAX_PHRASE_TOKENS} words; ignored"),
                    ));
                    continue;
                }
                t.phrase_max = t.phrase_max.max(toks.len());
                t.spell.insert(toks.join(" "), i);
            }
        }
    }

    for l in layers {
        t.hidden.extend(l.hidden.iter().copied());
    }
    (t, w)
}

fn resolve_phrase(e: &EmoDb, t: &OverlayTables, ph: &OverlayPhrase, key: &str, w: &mut Vec<Warning>) -> Option<UserPhrase> {
    let st = &e.st;
    if key.is_empty() {
        return None;
    }
    if let Some(target) = &ph.alias_of {
        let target = norm_term(target);
        let spelled = tokens(&target).join(" ");
        if let Some(&i) = t.spell.get(&spelled) {
            return Some(t.phrases[i].clone());
        }
        if let Some(v) = st.phrase_lookup(&spelled).and_then(|pi| st.phrase(pi)) {
            return Some(UserPhrase {
                key: v.key.to_string(),
                p: v.p,
                m: None,
                level: v.level,
                pair: v.pair,
                words: v.words.iter().map(|s| s.to_string()).collect(),
                cute: v.cute,
                lenny: v.lenny,
                lewd: v.lewd,
            });
        }
        if let Some(ti) = st.term_index(&target) {
            let r = st.term(ti);
            return Some(UserPhrase {
                key: target,
                p: r.p,
                m: r.m(),
                level: None,
                pair: None,
                words: Vec::new(),
                cute: false,
                lenny: false,
                lewd: false,
            });
        }
        w.push(Warning::new(
            "overlay_unknown_alias",
            Some("overlays"),
            format!("phrase {key:?}: alias_of {target:?} is not a known term or phrase; ignored"),
        ));
        return None;
    }
    let mut p = [0f32; N_EMO];
    for (name, v) in &ph.p {
        match e.emotions.iter().position(|x| x == name) {
            Some(i) => p[i] = if v.is_finite() { v.max(0.0) } else { 0.0 },
            None => w.push(Warning::new(
                "overlay_unknown_emotion",
                Some("overlays"),
                format!("phrase {key:?}: unknown emotion {name:?} ignored"),
            )),
        }
    }
    let s: f32 = p.iter().sum();
    if s > 0.0 {
        p.iter_mut().for_each(|x| *x /= s);
    } else if ph.words.is_empty() {
        w.push(Warning::new("overlay_bad_line", Some("overlays"), format!("phrase {key:?} has no emotions or words; ignored")));
        return None;
    }
    Some(UserPhrase {
        key: key.to_string(),
        p,
        m: None,
        level: ph.level.filter(|l| (1..=3).contains(l)),
        pair: ph.pair,
        words: ph.words.iter().map(|x| norm_term(x)).filter(|x| !x.is_empty()).collect(),
        cute: ph.cute,
        lenny: ph.lenny,
        lewd: ph.suggestive,
    })
}

#[cfg(all(test, feature = "fs"))]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_file_names() {
        assert_eq!(OverlayKind::from_file_name("canonical.jsonl"), Some(OverlayKind::Canonical));
        assert_eq!(OverlayKind::from_file_name("canonical_hand.jsonl"), Some(OverlayKind::Canonical));
        assert_eq!(OverlayKind::from_file_name("lexicon_phrases.jsonl"), Some(OverlayKind::Phrases));
        assert_eq!(OverlayKind::from_file_name("boosts.jsonl"), Some(OverlayKind::Boosts));
        assert_eq!(OverlayKind::from_file_name("blocklist.txt"), Some(OverlayKind::Blocklist));
        assert_eq!(OverlayKind::from_file_name("canonicalx.jsonl"), None);
        assert_eq!(OverlayKind::from_file_name("usage.json"), None);
    }

    #[test]
    fn parses_every_kind_and_warns_on_bad_lines() {
        let (o, w) = Overlay::parse(
            OverlayKind::Canonical,
            "{\"term\":\"Shrug \",\"text\":\"(^‿^)\",\"rank\":2}\n\n{\"term\":\"happy\",\"clear\":true}\n{\"term\":\"x\"}\nnope\n[1]\n{\"term\":\"y\",\"id\":\"k2026da3e4989\",\"rank\":0}\n",
            "c",
        );
        assert_eq!(o.pins, vec![CanonPin { term: "shrug".into(), id: FaceId::of_text("(^‿^)"), rank: 2 }]);
        assert!(o.clear.contains("happy"));
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w.iter().all(|w| w.code == "overlay_bad_line"));
        assert!(w[0].message.starts_with("c:4: "), "{}", w[0].message);

        // emoticond-state's format: bare hex ids, an i8 boost and a why
        let (o, w) = Overlay::parse(
            OverlayKind::Boosts,
            "{\"term\":\"shrug\",\"id\":\"2026da3e4989\",\"boost\":-3,\"why\":\"doesn't fit (report)\"}\n{\"term\":\"shrug\",\"id\":\"k2026da3e4989\",\"boost\":9}\n{\"term\":\"shrug\",\"id\":\"zz\"}\n",
            "b",
        );
        assert_eq!(o.boosts.len(), 2);
        assert_eq!(o.boosts[0].id, o.boosts[1].id);
        assert_eq!((o.boosts[0].boost, o.boosts[1].boost), (-3.0, 3.0));
        assert_eq!(w.iter().map(|w| w.code.as_ref()).collect::<Vec<_>>(), ["overlay_out_of_range", "overlay_bad_line"]);

        let (o, w) = Overlay::parse(
            OverlayKind::Phrases,
            "{\"key\":\"blep\",\"match\":[\"bleps\"],\"p\":{\"playful\":2},\"attr\":[\"cute\"],\"level\":1}\n{\"key\":\"meh\",\"alias_of\":\"shrug\"}\n{\"key\":\"x\"}\n{\"key\":\"y\",\"p\":{\"sad\":\"lots\"}}\n",
            "p",
        );
        assert_eq!(o.phrases.len(), 2);
        assert!(o.phrases[0].cute && o.phrases[0].level == Some(1) && o.phrases[0].spellings == ["bleps"]);
        assert_eq!(o.phrases[1].alias_of.as_deref(), Some("shrug"));
        assert_eq!(w.len(), 2);

        let (o, w) = Overlay::parse(OverlayKind::Blocklist, "# hidden\nk2026da3e4989\n(^‿^)\n#_#\nkbogus\n", "l");
        assert_eq!(o.hidden.len(), 3);
        assert!(o.hidden.contains(&FaceId::of_text("#_#")));
        assert_eq!(w.len(), 1);
    }
}
