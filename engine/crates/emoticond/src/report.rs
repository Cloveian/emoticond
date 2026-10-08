//! Feedback reports (docs/options.md §5, docs/api-frontends.md §1.2).
//!
//! One record type for both menus. The library builds and validates a
//! report and says what its local effects are; storing, superseding by
//! `key` and sending belong to `emoticond-state`. The caller supplies the
//! report id, time and disclaimer version, since the library has no clock or
//! RNG.
//!
//! A report carries the raw query, the reading line, the menu choice, the
//! face if any, the top 20 faces shown, the safety and
//! styles in force, and engine/data versions. Nothing else: no usage
//! history, install id, locale or hostname.

use crate::error::ReportError;
use crate::id::FaceId;
use crate::options::{Safety, Styles};
use crate::result::{Hit, SearchResult};
use serde::{Deserialize, Serialize};

/// Record schema version.
pub const REPORT_VERSION: u16 = 1;
/// How many shown faces a report carries.
pub const SHOWN_MAX: usize = 20;
/// Default cap on the free-text note, in chars (`feedback.custom_max_chars`).
pub const NOTE_MAX_CHARS: usize = 500;

/// What the report is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Target {
    /// The query menu: how the whole query was read and answered.
    Query,
    /// The face menu: one result.
    Face { id: FaceId, text: String, rank: u32 },
}

/// The menu choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Reason {
    // query menu
    /// "interpreted my search well"
    ReadWell,
    /// "interpreted it incorrectly"
    ReadWrong,
    /// "didn't return what I wanted"
    Missing,
    // face menu
    /// "really good fit": also a pick for popularity.
    GreatFit,
    /// "fits, but not the word I used": report only.
    OtherWord,
    /// "doesn't fit": demotes the face for this concept locally.
    NoFit,
    /// "offensive / explicit": blocklisted and hidden at once, always.
    Offensive,
    // either menu
    /// Custom text only.
    Note,
    /// Withdraws the earlier report with the same key.
    Clear,
}

impl Reason {
    /// A face-menu reason.
    pub fn needs_face(self) -> bool {
        matches!(self, Reason::GreatFit | Reason::OtherWord | Reason::NoFit | Reason::Offensive)
    }
    /// A query-menu reason.
    pub fn needs_query(self) -> bool {
        matches!(self, Reason::ReadWell | Reason::ReadWrong | Reason::Missing)
    }
}

/// Engine and data versions a report was made with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineStamp {
    pub engine: String,
    pub data: String,
}

/// One feedback report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Report {
    /// Record schema version ([`REPORT_VERSION`]).
    pub v: u16,
    /// Random, caller-supplied.
    pub report_id: String,
    /// Supersession key: [`report_key`] of (query, target). A later report
    /// with the same key replaces an earlier one; `Clear` withdraws it.
    pub key: String,
    /// Caller-supplied unix ms.
    pub ts: u64,
    pub target: Target,
    pub reason: Reason,
    /// For `Clear`: which choice it withdraws (its slot, see [`slot`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clears: Option<Reason>,
    pub note: Option<String>,
    pub query: String,
    pub term_key: Option<String>,
    pub reading: Option<String>,
    pub corrected: Option<String>,
    pub safety: Safety,
    pub styles: Styles,
    /// The top faces as displayed (at most [`SHOWN_MAX`]).
    pub shown: Vec<FaceId>,
    pub engine: EngineStamp,
    pub disclaimer_version: u32,
}

/// Which independent choice a reason belongs to. Reports in the same slot
/// replace each other; different slots stand side by side, so "interpreted
/// well" and "didn't return what I wanted" are both kept, as are "doesn't
/// fit" and "offensive" on one face. `Clear` has no slot of its own: it
/// takes the slot of the reason it `clears`.
pub fn slot(reason: Reason) -> &'static str {
    match reason {
        Reason::ReadWell | Reason::ReadWrong => "verdict",
        Reason::Missing => "missing",
        Reason::GreatFit | Reason::OtherWord | Reason::NoFit => "fit",
        Reason::Offensive => "offensive",
        Reason::Note => "note",
        Reason::Clear => "",
    }
}

/// The supersession key of a report: 16 hex digits of SHA-1 over the
/// trimmed, lowercased query, the target (`"query"` or the face id) and the
/// slot of `reason` (of `clears` for a `Clear`).
pub fn report_key(query: &str, target: &Target, reason: Reason, clears: Option<Reason>) -> String {
    let q = query.trim().to_lowercase();
    let t = match target {
        Target::Query => "query".to_string(),
        Target::Face { id, .. } => id.to_string(),
    };
    let mut h = sha1_smol::Sha1::new();
    h.update(q.as_bytes());
    h.update(&[0]);
    h.update(t.as_bytes());
    h.update(&[0]);
    h.update(slot(if reason == Reason::Clear { clears.unwrap_or(Reason::Clear) } else { reason }).as_bytes());
    h.digest().to_string()[..16].to_string()
}

/// Something the front-end should do on this machine right away, whether or
/// not the report is ever sent (options.md §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum LocalEffect {
    /// Add the face to the user's blocklist and exclude it at once. `text`
    /// is the face, for a human-readable comment beside the id.
    Block { id: FaceId, text: Option<String> },
    /// Count it as a pick for popularity (if popularity is on).
    RecordPick { id: FaceId, term_key: Option<String> },
    /// A negative boost for the face under this concept, in the machine
    /// overlay.
    Demote { id: FaceId, term_key: String },
}

impl LocalEffect {
    /// Applied even with `feedback.apply_locally = false` (the offensive hide).
    pub fn is_mandatory(&self) -> bool {
        matches!(self, LocalEffect::Block { .. })
    }
}

/// Builds and validates a [`Report`].
#[derive(Debug, Clone)]
pub struct ReportBuilder {
    target: Target,
    reason: Reason,
    clears: Option<Reason>,
    note: Option<String>,
    query: String,
    term_key: Option<String>,
    reading: Option<String>,
    corrected: Option<String>,
    safety: Safety,
    styles: Styles,
    shown: Vec<FaceId>,
    engine: EngineStamp,
    stamp: Option<(String, u64, u32)>,
    note_max: usize,
}

impl Report {
    /// A query-menu report on `result`, the answer to `query`.
    pub fn for_query(result: &SearchResult, query: &str, reason: Reason) -> ReportBuilder {
        ReportBuilder::from_result(result, query, Target::Query, reason)
    }

    /// A face-menu report on `hit`, one of `result`'s hits.
    pub fn for_face(result: &SearchResult, query: &str, hit: &Hit, reason: Reason) -> ReportBuilder {
        let target = Target::Face { id: hit.id, text: hit.text.clone(), rank: hit.rank };
        ReportBuilder::from_result(result, query, target, reason)
    }
}

impl ReportBuilder {
    /// A report without a `SearchResult` at hand (for example rebuilt from a
    /// front-end's own state). Set the rest with the setters.
    pub fn new(query: &str, target: Target, reason: Reason) -> ReportBuilder {
        ReportBuilder {
            target,
            reason,
            clears: None,
            note: None,
            query: query.to_string(),
            term_key: None,
            reading: None,
            corrected: None,
            safety: Safety::default(),
            styles: Styles::default(),
            shown: Vec::new(),
            engine: EngineStamp { engine: env!("CARGO_PKG_VERSION").to_string(), data: String::new() },
            stamp: None,
            note_max: NOTE_MAX_CHARS,
        }
    }

    fn from_result(result: &SearchResult, query: &str, target: Target, reason: Reason) -> ReportBuilder {
        let mut b = ReportBuilder::new(query, target, reason);
        b.term_key = result.term_key.clone();
        b.reading = result.reading.as_ref().map(|r| r.line.clone());
        b.corrected = result.corrected.clone();
        b.safety = result.safety;
        b.styles = result.styles;
        b.shown = result.hits.iter().take(SHOWN_MAX).map(|h| h.id).collect();
        b
    }

    /// For `Clear`: the choice being withdrawn (sets the key's slot).
    pub fn clears(mut self, r: Reason) -> Self {
        self.clears = Some(r);
        self
    }
    pub fn note(mut self, s: impl Into<String>) -> Self {
        self.note = Some(s.into());
        self
    }
    /// The caller's random id, unix ms and the disclaimer version shown.
    pub fn stamp(mut self, report_id: impl Into<String>, ts: u64, disclaimer_version: u32) -> Self {
        self.stamp = Some((report_id.into(), ts, disclaimer_version));
        self
    }
    /// The data set's version (the engine version is filled in).
    pub fn data_version(mut self, data: impl Into<String>) -> Self {
        self.engine.data = data.into();
        self
    }
    pub fn reading(mut self, line: Option<String>) -> Self {
        self.reading = line;
        self
    }
    pub fn term_key(mut self, k: Option<String>) -> Self {
        self.term_key = k;
        self
    }
    pub fn in_force(mut self, safety: Safety, styles: Styles) -> Self {
        self.safety = safety;
        self.styles = styles;
        self
    }
    /// The faces shown, best first (only the first 20 are kept).
    pub fn shown(mut self, ids: impl IntoIterator<Item = FaceId>) -> Self {
        self.shown = ids.into_iter().take(SHOWN_MAX).collect();
        self
    }
    /// The note limit in chars (`feedback.custom_max_chars`, default 500).
    pub fn max_note_chars(mut self, n: usize) -> Self {
        self.note_max = n;
        self
    }

    pub fn reason(&self) -> Reason {
        self.reason
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn target(&self) -> &Target {
        &self.target
    }
    pub fn has_clears(&self) -> bool {
        self.clears.is_some()
    }

    /// The local effects of this report, as data the caller applies.
    pub fn local_effects(&self) -> Vec<LocalEffect> {
        let Target::Face { id, text, .. } = &self.target else { return Vec::new() };
        match self.reason {
            Reason::Offensive => vec![LocalEffect::Block { id: *id, text: Some(text.clone()).filter(|t| !t.is_empty()) }],
            Reason::GreatFit => vec![LocalEffect::RecordPick { id: *id, term_key: self.term_key.clone() }],
            Reason::NoFit => match &self.term_key {
                Some(k) => vec![LocalEffect::Demote { id: *id, term_key: k.clone() }],
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Validate and build.
    pub fn build(self) -> Result<Report, ReportError> {
        let note = self.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        check(&self.target, self.reason, note.as_deref(), self.note_max)?;
        let Some((report_id, ts, disclaimer_version)) = self.stamp else { return Err(ReportError::NotStamped) };
        Ok(Report {
            v: REPORT_VERSION,
            report_id,
            key: report_key(&self.query, &self.target, self.reason, self.clears),
            ts,
            target: self.target,
            reason: self.reason,
            clears: if self.reason == Reason::Clear { self.clears } else { None },
            note,
            query: self.query,
            term_key: self.term_key,
            reading: self.reading,
            corrected: self.corrected,
            safety: self.safety,
            styles: self.styles,
            shown: self.shown,
            engine: self.engine,
            disclaimer_version,
        })
    }
}

/// The checks `build()` makes, shared with `Report::validate`.
fn check(target: &Target, reason: Reason, note: Option<&str>, note_max: usize) -> Result<(), ReportError> {
    let is_face = matches!(target, Target::Face { .. });
    if reason.needs_face() && !is_face {
        return Err(ReportError::ReasonNeedsFace);
    }
    if reason.needs_query() && is_face {
        return Err(ReportError::ReasonNeedsQuery);
    }
    if reason == Reason::Note && note.is_none_or(|n| n.trim().is_empty()) {
        return Err(ReportError::NoteRequired);
    }
    if note.is_some_and(|n| n.chars().count() > note_max) {
        return Err(ReportError::NoteTooLong { max: note_max });
    }
    Ok(())
}

impl Report {
    /// Check a report that came from elsewhere (a queue file): the same
    /// checks as `ReportBuilder::build` (note limit [`NOTE_MAX_CHARS`]),
    /// plus a `key` that matches its query and target and at most
    /// [`SHOWN_MAX`] shown faces.
    pub fn validate(&self) -> Result<(), ReportError> {
        self.validate_with(NOTE_MAX_CHARS)
    }

    /// `validate` with a different note limit (`feedback.custom_max_chars`).
    pub fn validate_with(&self, note_max: usize) -> Result<(), ReportError> {
        check(&self.target, self.reason, self.note.as_deref(), note_max)?;
        if self.key != report_key(&self.query, &self.target, self.reason, self.clears) {
            return Err(ReportError::KeyMismatch);
        }
        if self.shown.len() > SHOWN_MAX {
            return Err(ReportError::TooManyShown { max: SHOWN_MAX });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{Flags, Reading};

    fn result() -> SearchResult {
        let hits = (0..25)
            .map(|i| Hit {
                id: FaceId::of_text(&i.to_string()),
                text: i.to_string(),
                score: 1.0,
                rank: i,
                flags: Flags::empty(),
                why: None,
            })
            .collect();
        SearchResult {
            hits,
            term_key: Some("shrug".into()),
            reading: Some(Reading { line: "shrug · pinned faces first".into(), ..Reading::default() }),
            safety: Safety::Moderate,
            ..SearchResult::default()
        }
    }

    #[test]
    fn query_report_carries_context() {
        let r = result();
        let rep = Report::for_query(&r, "shrug", Reason::ReadWell).stamp("r1", 99, 2).build().unwrap();
        assert_eq!(rep.shown.len(), SHOWN_MAX);
        assert_eq!(rep.shown[0], r.hits[0].id);
        assert_eq!(rep.reading.as_deref(), Some("shrug · pinned faces first"));
        assert_eq!(rep.term_key.as_deref(), Some("shrug"));
        assert_eq!(rep.safety, Safety::Moderate);
        assert_eq!((rep.ts, rep.disclaimer_version, rep.v), (99, 2, REPORT_VERSION));
        assert_eq!(rep.key, report_key(" Shrug ", &Target::Query, rep.reason, None));
        let j = serde_json::to_string(&rep).unwrap();
        assert_eq!(serde_json::from_str::<Report>(&j).unwrap(), rep);
    }

    #[test]
    fn validation() {
        let r = result();
        let h = &r.hits[3];
        let e = |b: ReportBuilder| b.stamp("x", 0, 1).build().unwrap_err();
        assert_eq!(e(Report::for_query(&r, "q", Reason::GreatFit)), ReportError::ReasonNeedsFace);
        assert_eq!(e(Report::for_face(&r, "q", h, Reason::Missing)), ReportError::ReasonNeedsQuery);
        assert_eq!(e(Report::for_query(&r, "q", Reason::Note).note("  ")), ReportError::NoteRequired);
        assert_eq!(
            e(Report::for_face(&r, "q", h, Reason::Note).note("abcdef").max_note_chars(5)),
            ReportError::NoteTooLong { max: 5 }
        );
        assert_eq!(Report::for_query(&r, "q", Reason::Clear).build().unwrap_err(), ReportError::NotStamped);
        assert!(Report::for_face(&r, "q", h, Reason::Clear).stamp("x", 0, 1).build().is_ok());
        assert!(Report::for_face(&r, "q", h, Reason::Note).note("hm").stamp("x", 0, 1).build().is_ok());
    }

    #[test]
    fn validate_checks_stored_reports() {
        let r = result();
        let rep = Report::for_face(&r, "idk", &r.hits[0], Reason::Note).note("hm").stamp("1", 0, 1).build().unwrap();
        assert_eq!(rep.validate(), Ok(()));
        let mut bad = rep.clone();
        bad.query = "other".into();
        assert_eq!(bad.validate(), Err(ReportError::KeyMismatch));
        let mut bad = rep.clone();
        bad.note = None;
        assert_eq!(bad.validate(), Err(ReportError::NoteRequired));
        assert_eq!(rep.validate_with(1), Err(ReportError::NoteTooLong { max: 1 }));
        let mut bad = rep.clone();
        bad.reason = Reason::Missing;
        assert_eq!(bad.validate(), Err(ReportError::ReasonNeedsQuery));
        let mut bad = rep;
        bad.shown = vec![bad.shown[0]; 21];
        assert_eq!(bad.validate(), Err(ReportError::TooManyShown { max: SHOWN_MAX }));
    }

    #[test]
    fn keys_separate_targets_and_ignore_case() {
        let r = result();
        let a = Report::for_face(&r, "idk", &r.hits[0], Reason::NoFit).stamp("1", 0, 1).build().unwrap();
        let b = Report::for_face(&r, "IDK ", &r.hits[0], Reason::GreatFit).stamp("2", 0, 1).build().unwrap();
        let c = Report::for_face(&r, "idk", &r.hits[1], Reason::NoFit).stamp("3", 0, 1).build().unwrap();
        assert_eq!(a.key, b.key);
        assert_ne!(a.key, c.key);
        assert_eq!(a.key.len(), 16);
    }

    #[test]
    fn local_effects() {
        let r = result();
        let h = &r.hits[2];
        let fx = |reason| Report::for_face(&r, "shrug", h, reason).local_effects();
        assert_eq!(fx(Reason::Offensive), vec![LocalEffect::Block { id: h.id, text: Some(h.text.clone()) }]);
        assert!(fx(Reason::Offensive)[0].is_mandatory());
        assert_eq!(fx(Reason::GreatFit), vec![LocalEffect::RecordPick { id: h.id, term_key: Some("shrug".into()) }]);
        assert_eq!(fx(Reason::NoFit), vec![LocalEffect::Demote { id: h.id, term_key: "shrug".into() }]);
        assert!(fx(Reason::OtherWord).is_empty());
        assert!(Report::for_query(&r, "shrug", Reason::Missing).local_effects().is_empty());
        let no_term = SearchResult { term_key: None, ..result() };
        assert!(Report::for_face(&no_term, "x", h, Reason::NoFit).local_effects().is_empty());
    }
}
