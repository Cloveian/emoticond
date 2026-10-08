//! Result shapes shared by the protocol (docs/protocol.md) and the CLI's
//! output formats (docs/api-frontends.md §2.2).

use emoticond::{Attrs, Database, Entry, Flags, Hit, HitWhy, Reading, SearchResult, Warning};
use serde::Serialize;
use std::collections::BTreeMap;

/// Extra per-hit fields a client asks for (`"fields"`). Opt-in, because the
/// Quickshell picker parses on the UI thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fields {
    pub flags: bool,
    pub emotions: bool,
    pub emotions_all: bool,
    pub quality: bool,
    pub attrs: bool,
    pub canonical_for: bool,
    pub why: bool,
}

impl Fields {
    pub const NAMES: &'static [&'static str] = &["flags", "emotions", "emotions_all", "quality", "attrs", "canonical_for", "why"];

    /// Parse a list of field names; unknown ones become warnings.
    pub fn parse<'a>(names: impl IntoIterator<Item = &'a str>, warnings: &mut Vec<Warning>) -> Fields {
        let mut f = Fields::default();
        for n in names {
            match n {
                "flags" => f.flags = true,
                "emotions" => f.emotions = true,
                "emotions_all" => f.emotions_all = true,
                "quality" => f.quality = true,
                "attrs" => f.attrs = true,
                "canonical_for" => f.canonical_for = true,
                "why" => f.why = true,
                other => warnings.push(Warning::new(
                    "unknown_field",
                    Some(other),
                    format!("unknown field {other:?} ignored (known: {})", Fields::NAMES.join(", ")),
                )),
            }
        }
        f
    }

    fn needs_entry(self) -> bool {
        self.emotions || self.emotions_all || self.quality || self.attrs || self.canonical_for
    }
}

/// Flag names, lowercase, in bit order.
pub fn flag_names(f: Flags) -> Vec<&'static str> {
    [
        (Flags::SUGGESTIVE, "suggestive"),
        (Flags::EXPLICIT, "explicit"),
        (Flags::LENNY, "lenny"),
        (Flags::CRUDE, "crude"),
        (Flags::MULTI, "multi"),
        (Flags::NOT_FACE, "not_face"),
        (Flags::PINNED, "pinned"),
        (Flags::LONG, "long"),
    ]
    .into_iter()
    .filter(|(b, _)| f.contains(*b))
    .map(|(_, n)| n)
    .collect()
}

/// A score rounded to 4 decimals.
pub fn round4(x: f32) -> f64 {
    (f64::from(x) * 1e4).round() / 1e4
}

fn emotion_map(v: &[(String, f32)]) -> BTreeMap<String, f64> {
    v.iter().map(|(k, x)| (k.clone(), (f64::from(*x) * 1e3).round() / 1e3)).collect()
}

/// A face's graded attributes, rounded to 3 decimals.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AttrsOut {
    pub multi: f64,
    pub cute: f64,
    pub intensity: f64,
    /// 0..=3: about 1 innuendo, 2 sexual, 3 explicit.
    pub suggestive: f64,
    pub lenny: f64,
    pub face: f64,
}

impl AttrsOut {
    pub fn of(a: &Attrs) -> AttrsOut {
        let r = |x: f32| (f64::from(x) * 1e3).round() / 1e3;
        AttrsOut { multi: r(a.multi), cute: r(a.cute), intensity: r(a.intensity), suggestive: r(a.suggestive), lenny: r(a.lenny), face: r(a.face) }
    }
}

/// One result on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct HitOut {
    /// The stable face id (`k` + 12 hex).
    pub id: String,
    pub text: String,
    pub kind: &'static str,
    pub score: f64,
    pub rank: u32,
    /// Browse only: `canonical` (a starter face) or `history` (your picks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<Vec<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotions: Option<BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<AttrsOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_for: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<HitWhy>,
}

impl HitOut {
    pub fn of(db: &Database, h: &Hit, f: Fields, browse: bool) -> HitOut {
        let entry = if f.needs_entry() { db.get(h.id) } else { None };
        let e = entry.as_ref();
        HitOut {
            id: h.id.to_string(),
            text: h.text.clone(),
            kind: "emoticon",
            score: round4(h.score),
            rank: h.rank,
            from: browse.then_some(if h.flags.contains(Flags::PINNED) { "canonical" } else { "history" }),
            flags: f.flags.then(|| flag_names(h.flags)),
            emotions: if f.emotions_all {
                e.map(|e| emotion_map(&e.emotions))
            } else if f.emotions {
                e.map(|e| emotion_map(&e.top_emotions(3)))
            } else {
                None
            },
            quality: if f.quality { e.map(|e| (f64::from(e.quality) * 100.0).round() / 100.0) } else { None },
            attrs: if f.attrs { e.map(|e| AttrsOut::of(&e.attrs)) } else { None },
            canonical_for: if f.canonical_for { e.map(|e| e.canonical_for.clone()) } else { None },
            why: if f.why { h.why.clone() } else { None },
        }
    }

    /// The `dmenu` hint: the concepts it is the canonical face for, else
    /// its top two emotions (`shrug · bored`).
    pub fn hint(db: &Database, id: emoticond::FaceId) -> String {
        let Some(e) = db.get(id) else { return String::new() };
        if !e.canonical_for.is_empty() {
            return e.canonical_for.iter().take(2).cloned().collect::<Vec<_>>().join(" · ");
        }
        e.top_emotions(2).into_iter().filter(|(_, v)| *v > 0.05).map(|(k, _)| k).collect::<Vec<_>>().join(" · ")
    }
}

/// A search, browse or similar response (protocol v1, and `--json`).
#[derive(Debug, Clone, Serialize)]
pub struct SearchOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub ok: bool,
    pub q: String,
    /// `search`, `browse` (empty query) or `similar`.
    pub mode: &'static str,
    pub corrected: Option<String>,
    pub term_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading: Option<Reading>,
    pub n: usize,
    pub counts: BTreeMap<&'static str, usize>,
    pub results: Vec<HitOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clamped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    pub ms: f64,
}

impl SearchOut {
    /// The response for a library result. `extra` are the request's own
    /// warnings (unknown keys, ...), listed first.
    pub fn of(db: &Database, q: &str, mode: &'static str, r: &SearchResult, f: Fields, extra: Vec<Warning>) -> SearchOut {
        let results: Vec<HitOut> = r.hits.iter().map(|h| HitOut::of(db, h, f, mode == "browse")).collect();
        let mut warnings = extra;
        warnings.extend(r.warnings.iter().cloned());
        let mut counts = BTreeMap::new();
        counts.insert("emoticon", results.len());
        SearchOut {
            id: None,
            ok: true,
            q: q.to_string(),
            mode,
            corrected: r.corrected.clone(),
            term_key: r.term_key.clone(),
            reading: r.reading.clone(),
            n: results.len(),
            counts,
            results,
            clamped: r.clamped.iter().map(|c| c.to_string()).collect(),
            warnings,
            ms: 0.0,
        }
    }
}

/// One face, for `get` (protocol and CLI).
#[derive(Debug, Clone, Serialize)]
pub struct EntryOut {
    pub id: String,
    pub text: String,
    pub quality: f64,
    pub flags: Vec<&'static str>,
    pub attrs: AttrsOut,
    /// Every emotion, 0..=1.
    pub emotions: BTreeMap<String, f64>,
    pub canonical_for: Vec<String>,
}

impl EntryOut {
    pub fn of(e: &Entry) -> EntryOut {
        EntryOut {
            id: e.id.to_string(),
            text: e.text.clone(),
            quality: (f64::from(e.quality) * 100.0).round() / 100.0,
            flags: flag_names(e.flags),
            attrs: AttrsOut::of(&e.attrs),
            emotions: emotion_map(&e.emotions),
            canonical_for: e.canonical_for.clone(),
        }
    }
}

/// `{"code","key"?,"message"}` for a protocol error.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorOut {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub message: String,
}

/// JSON for a value that always serialises.
pub fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":{{\"code\":\"internal\",\"message\":{:?}}}}}", e.to_string()))
}

/// A JSON object written in insertion order (serde_json's `Map` sorts its
/// keys in this build), so responses read `{"id":..,"ok":..,...}`.
#[derive(Debug, Clone, Default)]
pub struct Obj(Vec<(String, String)>);

impl Obj {
    pub fn new() -> Obj {
        Obj::default()
    }

    /// Add or replace `k`.
    pub fn put(mut self, k: &str, v: impl Serialize) -> Obj {
        self.set(k, v);
        self
    }

    /// Add `k` only when `cond`.
    pub fn put_if(self, cond: bool, k: &str, v: impl Serialize) -> Obj {
        if cond {
            self.put(k, v)
        } else {
            self
        }
    }

    pub fn set(&mut self, k: &str, v: impl Serialize) {
        let v = serde_json::to_string(&v).unwrap_or_else(|_| "null".into());
        match self.0.iter_mut().find(|(x, _)| x == k) {
            Some(slot) => slot.1 = v,
            None => self.0.push((k.to_string(), v)),
        }
    }

    /// Add already-serialised JSON (an `Obj`, keeping its order).
    /// The `"k":v,...` part, for splicing into another object.
    pub fn inner(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{}:{v}", json(k))).collect::<Vec<_>>().join(",")
    }
}

impl std::fmt::Display for Obj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{{}}}", self.inner())
    }
}
