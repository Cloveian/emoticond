//! The emotion engine: every face ranked on graded emotion scores.
//!
//! Everything slow is precomputed by `pipeline/export_engine2.py` into
//! work/engine/: each face's 19 emotion intensities (0..1, normalised per
//! emotion so "a bit sorry" means what "a bit sad" means), multi-figure, cute,
//! intensity, suggestive, lenny, face, predicted quality and words; every
//! query term's emotion profile and expected share of multi-figure faces; and
//! the fine-tuned e5 model's top 100 faces per term. At query time this only
//! parses, looks up and does arithmetic over the faces.
//!
//! A query is parsed into terms (vocab entries and hand-written situations,
//! with "a bit" / "very" / "not" and -ly adverbs attached) plus lexical words
//! and flags. A face's score is
//!
//! ```text
//!   emotion match    the weakest of the main terms (several words mean "all
//!                    of these"; a sentence averages instead), plus a little
//!                    for adverbs
//!   + dense          how high e5 ranked it for the terms
//!   + lexical        its tags or labels contain a query word
//!   + figures        pairs for hug / comfort / kiss, single for "face" or
//!                    plain emotions
//!   + quality        the visual rating model
//!   + boost          the curated canonical faces
//!   - penalties      not a face, long, suggestive (0.05 x 4^(n-1), a bonus
//!                    when asked for), lenny unless asked, crude
//!   + usage          the caller's personal popularity (SearchOptions::usage)
//! ```
//!
//! then sorted and thinned of near-duplicates. A query that is one plain
//! concept (`idk`, `shrug`, `table flip`, `pat pat`) puts its canonical faces
//! first -- data/canonical.jsonl, picked per query by the search-judge agent
//! from candidates that include the faces people actually use most -- because
//! which shrug is *the* shrug is a fact about usage, not about the face.
//!
//! How each part applies is set by the query's [`SearchOptions`], resolved
//! into a [`Ctx`]. With `SearchOptions::legacy` the arithmetic is exactly the
//! pre-library engine's, term for term and in the same order, so scores are
//! bit-identical.
//!
//! Everything comes from the data file (data/): faces, vocab, neighbour
//! lists, postings, and the compiled phrases, situations, canonical picks,
//! boosts, grammar word lists and safety thresholds. Nothing is parsed at
//! open.

use crate::data::format::FaceNum;
use crate::data::DataFile;
use crate::grammar::Grammar;
use crate::id::FaceId;
use crate::data::PhraseView;
use crate::options::{Figures, Intensity, Safety, SearchOptions, StyleMode, UsageWeight};
use crate::overlay::OverlayTables;
use crate::result::{HitWhy, Modifier, ReadMode, ReadTerm, Reading};
use crate::text::jaccard;
use std::collections::{HashMap, HashSet};

pub const N_EMO: usize = crate::data::format::N_EMO;

/// Weights of the parts of a face's score.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Weights {
    pub emo: f32,
    pub dense: f32,
    pub engine: f32,
    pub lex: f32,
}
// Tuned on half the judged queries, checked on the other half (0.31 vs 0.18 for
// the first guess); the lists carry single words, the emotion part combinations.
pub(crate) const DEFAULT_WEIGHTS: Weights = Weights { emo: 0.5, dense: 1.2, engine: 1.2, lex: 0.2 };

const W_ADVERB: f32 = 0.4;
const W_QUALITY: f32 = 0.06;
const W_BOOST: f32 = 0.08;
const W_CUTE: f32 = 0.2;
const MIN_PRESENT: f32 = 0.08;
const LOW_TARGET: f32 = 0.35;
/// Bonus for a query's canonical faces, by rank: larger than any other part
/// of a score, so they always lead and in their own order.
const CANON_BONUS: [f32; 3] = [30.0, 20.0, 10.0];
/// Usage lift by `usage_weight` (a weight of 1 adds this much). `High` can
/// beat the largest curated boost (3 x W_BOOST = 0.24), never a pinned face.
const USAGE_LIFT: [f32; 4] = [0.0, 0.06, 0.15, 0.3];
/// The spread of `variety` jitter at variety = 1.
const JITTER: f32 = 0.4;

/// One resolved part of a query.
#[derive(Clone)]
pub struct QTerm {
    pub name: String,
    p: [f32; N_EMO],
    m: Option<f32>,
    dense: Option<usize>,
    weight: f32,
    target: Option<f32>,
    neg: bool,
    high: bool,
    pair: Option<bool>,
    /// Brings its e5 neighbours but asks for no emotion: a situation's
    /// words (`popcorn` for movie night), so untagged faces e5 knows still come.
    dense_only: bool,
    /// Key into the canonical table: the vocab term or the situation's name.
    canon: Option<String>,
    situation: bool,
    /// `emotions.target`: per-emotion target intensities (p holds the weights).
    per: Option<[f32; N_EMO]>,
    /// a phrase from the user's overlay
    user: bool,
    /// `target` or `high` comes from the phrase's own `level` (1 or 3), not
    /// from a word the user typed: the concept's pins still lead
    soft_level: bool,
}

impl QTerm {
    fn plain(name: String, p: [f32; N_EMO]) -> QTerm {
        QTerm {
            name,
            p,
            m: None,
            dense: None,
            weight: 1.0,
            target: None,
            neg: false,
            high: false,
            pair: None,
            dense_only: false,
            canon: None,
            situation: false,
            per: None,
            user: false,
            soft_level: false,
        }
    }
}

#[derive(Default)]
pub struct Parsed {
    pub terms: Vec<QTerm>,
    pub lexical: Vec<String>,
    pub want_pair: bool,
    pub want_single: bool,
    pub lewd: bool,
    pub lenny: bool,
    pub cute: bool,
    /// The user typed `cute` (not a phrase that carries the attribute): the
    /// bare concept's pins step aside so cuteness can reorder the results.
    pub cute_typed: bool,
    /// The last word is unknown and was not corrected: search-as-you-type
    /// may complete it (`very sa`, `im so ti`).
    pub trailing_unknown: bool,
    pub sentence: bool,
    pub corrected: Option<String>,
    pub boost_term: Option<String>,
    /// Set when the query is one plain concept: its canonical faces lead.
    pub canon_key: Option<String>,
    /// The parsed concept that keys usage and reports.
    pub term_key: Option<String>,
    /// `term_key` comes from the user's overlay (a user phrase, or a whole
    /// query only the user pinned faces for).
    pub term_user: bool,
}

/// A phrase: compiled into the data, or from the user's overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PRef {
    Shipped(usize),
    User(usize),
}

/// A query's options, resolved against the data (emotion names to indices,
/// face ids to ordinals) once per query.
pub(crate) struct Ctx<'a> {
    pub w: Weights,
    pub limit: usize,
    pub offset: usize,
    pub safety: Safety,
    pub lenny: StyleMode,
    pub crude: StyleMode,
    pub long: StyleMode,
    pub long_at: u32,
    pub max_len: Option<u32>,
    pub faces_only: bool,
    pub min_quality: Option<f32>,
    pub figures: Figures,
    pub intensity: Intensity,
    /// (weights, targets) from `emotions.target`
    pub target: Option<([f32; N_EMO], [f32; N_EMO])>,
    pub emo_min: Vec<(usize, f32)>,
    pub emo_max: Vec<(usize, f32)>,
    /// `emotions.min` above `emotions.max`: nothing can match.
    pub impossible: bool,
    pub dedupe: f32,
    pub pinned: bool,
    pub usage: &'a crate::usage::UsageMap,
    pub usage_lift: f32,
    pub variety: f32,
    pub seed: u64,
    /// the blocklist and the overlays' hidden faces, as ordinals
    pub blocked: &'a HashSet<u32>,
    /// the user's overlays (empty: the shipped data alone)
    pub ov: &'a OverlayTables,
    pub exclude: HashSet<u32>,
    pub correct: bool,
    pub complete_partial: bool,
    pub glyph: bool,
    pub full: bool,
    /// ties by export position instead of FaceId (Tuning::legacy_ties)
    pub legacy_ties: bool,
    /// any hard filter is on (a fast path for the common case)
    filters: bool,
}

impl<'a> Ctx<'a> {
    /// Resolve validated, policy-clamped options. Unknown emotion names are
    /// returned for the caller to warn about.
    pub fn new(
        db: &EmoDb,
        o: &'a SearchOptions,
        blocked: &'a HashSet<u32>,
        ov: &'a OverlayTables,
        unknown: &mut Vec<(&'static str, String)>,
    ) -> Ctx<'a> {
        #[cfg(not(feature = "unstable-tuning"))]
        let (w, legacy_ties) = (DEFAULT_WEIGHTS, false);
        #[cfg(feature = "unstable-tuning")]
        let legacy_ties = o.tuning.as_ref().is_some_and(|t| t.legacy_ties);
        #[cfg(feature = "unstable-tuning")]
        let w = match o.tuning.as_ref().and_then(|t| t.weights) {
            Some(t) => Weights { emo: t[0], dense: t[1], engine: t[2], lex: t[3] },
            None => DEFAULT_WEIGHTS,
        };
        let mut idx = |key: &'static str, m: &std::collections::BTreeMap<String, f32>| -> Vec<(usize, f32)> {
            m.iter()
                .filter_map(|(k, v)| match db.emotions.iter().position(|e| e == k) {
                    Some(i) => Some((i, *v)),
                    None => {
                        unknown.push((key, k.clone()));
                        None
                    }
                })
                .collect()
        };
        let tv = idx("emotions.target", &o.emotions.target);
        let emo_min = idx("emotions.min", &o.emotions.min);
        let emo_max = idx("emotions.max", &o.emotions.max);
        let target = (!tv.is_empty()).then(|| {
            let (mut p, mut t) = ([0f32; N_EMO], [0f32; N_EMO]);
            for &(i, v) in &tv {
                p[i] = 1.0 / tv.len() as f32;
                t[i] = v;
            }
            (p, t)
        });
        let impossible = emo_min.iter().any(|&(i, lo)| emo_max.iter().any(|&(j, hi)| i == j && lo > hi));
        let exclude: HashSet<u32> = o.exclude.iter().filter_map(|id| db.st.by_fid(*id)).collect();
        let usage_lift = match o.usage_weight {
            UsageWeight::Off => USAGE_LIFT[0],
            UsageWeight::Low => USAGE_LIFT[1],
            UsageWeight::Normal => USAGE_LIFT[2],
            UsageWeight::High => USAGE_LIFT[3],
        };
        let filters = !blocked.is_empty()
            || !exclude.is_empty()
            || o.safety == Safety::Strict
            || o.styles.lenny == StyleMode::Hide
            || o.styles.crude == StyleMode::Hide
            || o.styles.long == StyleMode::Hide
            || o.max_len.is_some()
            || o.faces_only
            || o.min_quality.is_some()
            || !emo_min.is_empty()
            || !emo_max.is_empty();
        Ctx {
            w,
            limit: usize::from(o.limit),
            offset: o.offset as usize,
            safety: o.safety,
            lenny: o.styles.lenny,
            crude: o.styles.crude,
            long: o.styles.long,
            long_at: u32::from(o.long_at),
            max_len: o.max_len.map(u32::from),
            faces_only: o.faces_only,
            min_quality: o.min_quality,
            figures: o.figures,
            intensity: o.intensity,
            target,
            emo_min,
            emo_max,
            impossible,
            dedupe: o.dedupe.threshold(),
            pinned: o.pinned,
            usage: &o.usage,
            usage_lift,
            variety: o.variety,
            seed: o.seed,
            blocked,
            ov,
            exclude,
            correct: o.correct,
            complete_partial: o.complete_partial,
            glyph: o.glyph_search == crate::options::GlyphSearch::Auto,
            full: o.explain == crate::options::Explain::Full,
            legacy_ties,
            filters,
        }
    }
}

/// What a search found, before it becomes a `SearchResult`.
pub(crate) struct Found {
    /// (face ordinal, score, pinned, pinned by the user), best first, after offset.
    pub hits: Vec<(usize, f32, bool, bool)>,
    pub corrected: Option<String>,
    pub term_key: Option<String>,
    pub term_user: bool,
    pub why: Vec<Option<HitWhy>>,
}

/// Per-query arrays shared by every face's score.
struct Prep {
    canon: Vec<f32>,
    dense: Vec<f32>,
    engine: Vec<f32>,
    lex: Vec<f32>,
    main: Vec<usize>,
    adv: Vec<usize>,
    high: bool,
    w: Weights,
    want_pair: bool,
    want_single: bool,
    pref: f32,
    /// ordinal -> (usage weight, used for this concept)
    usage: HashMap<u32, (f32, bool)>,
    boosts: Option<HashMap<FaceId, f32>>,
}

pub struct EmoDb {
    /// the data file: faces, vocab terms, neighbour lists, tag postings,
    /// phrases, situations, canonical picks and boosts
    pub(crate) st: DataFile,
    pub(crate) emotions: Vec<String>,
    grammar: Grammar,
    /// suggestive at or above this is innuendo: penalised unless asked for
    pub(crate) innuendo_at: f32,
    /// suggestive at or above this is hidden under `Safety::Strict`
    pub(crate) sexual_at: f32,
    /// crude at or above this counts as crude
    pub(crate) crude_at: f32,
}

/// Lowercased words with punctuation dropped (apostrophes kept).
pub fn tokens(text: &str) -> Vec<String> {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' || c == '$' { c } else { ' ' })
        .collect();
    t.split_whitespace().map(String::from).collect()
}

fn dot(a: &[f32; N_EMO], b: &[f32; N_EMO]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Text with symbols in it reads as pasted glyphs. Punctuation typed after
/// a word (`ok!`, `huh?`, `sorry...`) is not a symbol.
fn is_glyphy(t: &str) -> bool {
    let bare = t.trim_end_matches(['!', '?', '.', ',', ';', ':']);
    // punctuation alone (`!!`, `...`) is still pasted glyphs
    let t = if bare.is_empty() { t } else { bare };
    t.chars().any(|c| !c.is_alphanumeric() && !c.is_whitespace() && c != '-' && c != '\'')
        || t.chars().any(|c| !c.is_ascii() && !c.is_alphabetic())
}

/// A stretched word's plain forms, longest first: `noooooo` -> `noo`, `no`;
/// `yesss` -> `yess`, `yes`. None when nothing is stretched.
fn unstretch(w: &str) -> Option<[String; 2]> {
    let cs: Vec<char> = w.chars().collect();
    let mut two = String::new();
    let mut one = String::new();
    let mut run = 0;
    let mut stretched = false;
    for (i, &c) in cs.iter().enumerate() {
        run = if i > 0 && cs[i - 1] == c { run + 1 } else { 1 };
        if run >= 3 {
            stretched = true;
        }
        if run <= 2 {
            two.push(c);
        }
        if run == 1 {
            one.push(c);
        }
    }
    stretched.then_some([two, one])
}

/// splitmix64: the jitter's only randomness, seeded by the caller.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform in [0, 1) from the seed and the face id.
pub(crate) fn jitter_unit(seed: u64, id: FaceId) -> f32 {
    (splitmix64(seed ^ id.as_u64()) >> 40) as f32 / (1u64 << 24) as f32
}

impl EmoDb {
    /// The engine over an open data file.
    pub fn new(st: DataFile) -> EmoDb {
        let m = st.manifest();
        EmoDb {
            emotions: m.emotions.clone(),
            innuendo_at: m.innuendo_at,
            sexual_at: m.sexual_at,
            crude_at: m.crude_at,
            grammar: st.grammar().clone(),
            st,
        }
    }

    pub fn n_terms(&self) -> usize {
        self.st.n_terms
    }
    pub fn n_faces(&self) -> usize {
        self.st.n_faces
    }
    pub fn text(&self, i: usize) -> &str {
        self.st.text(i)
    }
    pub fn n_situations(&self) -> usize {
        self.st.n_situations()
    }
    pub fn n_canonical(&self) -> usize {
        self.st.n_canonical()
    }
    pub fn n_phrases(&self) -> usize {
        self.st.n_phrases()
    }

    /// The phrase a spelling names: the user's first, then the shipped one.
    pub(crate) fn phrase_find(&self, ov: &OverlayTables, s: &str) -> Option<PRef> {
        if !ov.spell.is_empty() {
            if let Some(&i) = ov.spell.get(s) {
                return Some(PRef::User(i));
            }
        }
        self.st.phrase_lookup(s).map(PRef::Shipped)
    }

    fn phrase_view<'a>(&'a self, ov: &'a OverlayTables, r: PRef) -> Option<PhraseView<'a>> {
        match r {
            PRef::Shipped(i) => self.st.phrase(i),
            PRef::User(i) => ov.phrases.get(i).map(|u| PhraseView {
                key: &u.key,
                p: u.p,
                level: u.level,
                pair: u.pair,
                words: u.words.iter().map(String::as_str).collect(),
                cute: u.cute,
                lenny: u.lenny,
                lewd: u.lewd,
                over: true,
                m: u.m,
                user: true,
            }),
        }
    }

    /// The longest phrase spelling, shipped or the user's, in tokens.
    fn phrase_max(&self, ov: &OverlayTables) -> usize {
        self.st.phrase_max().max(ov.phrase_max)
    }

    /// A concept's canonical faces after the overlays, best first:
    /// (ordinal, rank, pinned by the user).
    pub(crate) fn canonical(&self, ov: &OverlayTables, term: &str) -> Option<Vec<(u32, usize, bool)>> {
        if !ov.canon.is_empty() {
            if let Some(l) = ov.canon.get(term) {
                return Some(l.faces.iter().enumerate().map(|(r, &(f, u))| (f, r, u)).collect());
            }
        }
        self.st.canonical(term).map(|v| v.into_iter().map(|(f, r)| (f, r, false)).collect())
    }

    pub(crate) fn has_canonical(&self, ov: &OverlayTables, term: &str) -> bool {
        if !ov.canon.is_empty() {
            if let Some(l) = ov.canon.get(term) {
                return !l.faces.is_empty();
            }
        }
        self.st.has_canonical(term)
    }

    /// A generated-lexicon entry as a query term (plus the e5 neighbours of
    /// its tag words, which find faces that are tagged for it).
    fn push_phrase(&self, ov: &OverlayTables, out: &mut Parsed, pi: PRef, target: Option<f32>, neg: bool, high: bool) {
        let Some(ph) = self.phrase_view(ov, pi) else { return };
        let typed = high || target.is_some();
        let high = high || ph.level == Some(3);
        let soft_level = !typed && matches!(ph.level, Some(1) | Some(3));
        out.terms.push(QTerm {
            dense: self.st.term_index(ph.key),
            target: if high { None } else { target.or(if ph.level == Some(1) { Some(LOW_TARGET) } else { None }) },
            neg,
            high,
            pair: ph.pair,
            m: ph.m,
            canon: Some(ph.key.to_string()),
            user: ph.user,
            soft_level,
            ..QTerm::plain(ph.key.to_string(), ph.p)
        });
        out.cute |= ph.cute;
        out.lenny |= ph.lenny;
        out.lewd |= ph.lewd;
        out.lexical.extend(ph.words.iter().map(|w| w.to_string()));
        for w in ph.words {
            if let Some(ti) = self.st.term_index(w) {
                let mut q = self.qterm(w, ti);
                q.dense_only = true;
                q.m = None;
                out.terms.push(q);
            }
        }
    }

    fn qterm(&self, name: &str, i: usize) -> QTerm {
        let t = self.st.term(i);
        QTerm {
            m: t.m(),
            dense: Some(i),
            canon: Some(name.to_string()),
            ..QTerm::plain(name.to_string(), t.p)
        }
    }

    /// A vocab term the data knows too little about to outrank a phrase of
    /// the same spelling: an e5-only profile, a tag profile averaged from
    /// under 100 faces, or a tag+engine profile from under 50 (`eep` from
    /// 18 faces reads shy; the phrase says sleepy). Terms the data knows
    /// well (`cat`, `face`, `pouting` from hundreds of faces) keep their
    /// profiles unless the phrase says `over`; so do words with no tag at
    /// all whose profile e5 and the affect engine agree on (n = 0,
    /// `e5+engine`: the held-out lexicon keys the judged eval is built on).
    fn shadowed(&self, ti: usize) -> bool {
        let t = self.st.term(ti);
        t.weak() || (t.tier == 0 && t.n < 100) || (t.tier == 1 && t.n > 0 && t.n < 50)
    }

    /// The vocab term for a query word, except a single ASCII letter or
    /// digit the tags happen to carry (`o`, `a`, `x` from a dozen faces):
    /// those are not words, so `it was just o` completes `o` instead of
    /// reading it. A single CJK character (`冬`, `虫`) is a word.
    fn real_term(&self, w: &str) -> Option<usize> {
        let ti = self.st.term_index(w)?;
        let single = w.len() == 1 && self.st.term(ti).tier == 0;
        (!single).then_some(ti)
    }

    /// The phrase beats a vocab term of the same spelling regardless: the
    /// user's own, or a hand-written one marked `over`.
    fn phrase_over(&self, ov: &OverlayTables, pi: PRef) -> bool {
        matches!(pi, PRef::User(_)) || self.phrase_view(ov, pi).is_some_and(|p| p.over)
    }

    /// A vocab key the word starts with, when the leftover is a short suffix:
    /// `shruging` -> `shrug`. Tried before spelling correction, which turns
    /// unknown words into the wrong known ones (`kissing` -> `wishing`).
    /// `-y` only reaches a word the data knows beyond its tags (`pinky` is
    /// not `pink`).
    fn stem(&self, w: &str) -> Option<&str> {
        if w.chars().count() < 5 {
            return None;
        }
        let mut best: Option<&str> = None;
        for (k, i) in self.st.keys() {
            if k.len() >= 4 && w.starts_with(k) {
                // only real suffixes: `with` + `out` is not `with`
                let rest = &w[k.len()..];
                if ["s", "es", "ed", "d", "ing", "er", "ers", "y", "ly", "ish"].contains(&rest)
                    && !(rest == "y" && self.st.term(i).tier == 0)
                    && best.is_none_or(|b| k.len() > b.len())
                {
                    best = Some(k);
                }
            }
        }
        best
    }

    /// Nearest vocab key by edit distance, for a misspelled word. The cap
    /// grows with the word (1 to 3 edits) unless `cap` says otherwise (one
    /// edit inside a longer query). Ties go to the better-known key: emotion
    /// words before lexicon keys before tag phrases, then by face count; a
    /// tag seen on fewer than five faces is never a correction (`where` ->
    /// `whee`).
    fn correct<'a>(&'a self, ov: &'a OverlayTables, w: &str, cap: Option<usize>) -> Option<&'a str> {
        let wc: Vec<char> = w.chars().collect();
        if wc.len() <= 3 {
            return None;
        }
        let cap = cap.unwrap_or(if wc.len() <= 5 { 1 } else if wc.len() <= 8 { 2 } else { 3 });
        // (distance, tier, n): smaller distance, then higher tier, then more faces
        let mut best: Option<(usize, u32, u32, &str)> = None;
        let better = |d: usize, tier: u32, n: u32, b: &Option<(usize, u32, u32, &str)>| {
            b.is_none_or(|(bd, bt, bn, _)| d < bd || (d == bd && (tier > bt || (tier == bt && n > bn))))
        };
        for (k, i) in self.st.keys() {
            if k.len().abs_diff(w.len()) > cap || k.contains(' ') {
                continue;
            }
            let t = self.st.term(i);
            if t.tier == 0 && t.n < 5 {
                continue;
            }
            let kc: Vec<char> = k.chars().collect();
            let d = crate::text::damerau(&wc, &kc, cap);
            if d <= cap && better(d, t.tier, t.n, &best) {
                best = Some((d, t.tier, t.n, k));
            }
        }
        let user = if ov.spell.is_empty() { Vec::new() } else { ov.spellings() };
        for k in self.st.phrase_spellings().chain(user) {
            if k.contains(' ') || k.len().abs_diff(w.len()) > cap || self.st.term_index(k).is_some() {
                continue;
            }
            let kc: Vec<char> = k.chars().collect();
            let d = crate::text::damerau(&wc, &kc, cap);
            if d <= cap && best.is_none_or(|(bd, _, _, _)| d < bd) {
                best = Some((d, 1, 0, k));
            }
        }
        best.map(|(_, _, _, k)| k)
    }

    pub(crate) fn parse(&self, text: &str, ctx: &Ctx) -> Parsed {
        let g = &self.grammar;
        let ov = ctx.ov;
        let mut out = Parsed::default();
        let mut toks = tokens(text);

        // situations: whole-word phrase matches, removed from the query
        for s in self.st.situations() {
            for m in s.matches.iter().filter(|m| !m.is_empty()) {
                if m.len() > toks.len() {
                    continue;
                }
                if let Some(at) = (0..=toks.len() - m.len()).find(|&i| toks[i..i + m.len()].iter().zip(m).all(|(a, b)| a == b)) {
                    out.terms.push(QTerm {
                        pair: s.pair,
                        canon: Some(s.name.to_string()),
                        situation: true,
                        ..QTerm::plain(m.join(" "), s.p)
                    });
                    out.lexical.extend(s.words.iter().map(|w| w.to_string()));
                    for &w in &s.words {
                        if let Some(ti) = self.st.term_index(w) {
                            let mut q = self.qterm(w, ti);
                            q.dense_only = true;
                            q.m = None;
                            out.terms.push(q);
                        }
                    }
                    toks.drain(at..at + m.len());
                    break;
                }
            }
        }

        let (mut target, mut neg, mut high) = (None::<f32>, false, false);
        let mut content = 0usize;
        let mut i = 0;
        while i < toks.len() {
            let w = toks[i].as_str();
            // A multi-word phrase starting here wins before anything reads its
            // first word as a modifier or filler: "no thoughts head empty",
            // "this is fine", "i'm just a little guy".
            let mut multi = None;
            for n in (2..=self.phrase_max(ov)).rev() {
                if i + n <= toks.len() {
                    if let Some(pi) = self.phrase_find(ov, &toks[i..i + n].join(" ")) {
                        multi = Some((pi, n));
                        break;
                    }
                }
            }
            if let Some((pi, n)) = multi {
                let vocab_longer = (n + 1..=4).any(|m| i + m <= toks.len() && self.st.term_index(&toks[i..i + m].join(" ")).is_some());
                // the same words as a well-known vocab term (`so sad`, `run
                // away`): keep the vocab's tuned profile and lists
                let vocab_same = self
                    .st
                    .term_index(&toks[i..i + n].join(" "))
                    .is_some_and(|ti| !self.shadowed(ti));
                // the user's own phrase, or one marked `over`, wins over a
                // vocab term of the same words
                if !vocab_longer && (!vocab_same || self.phrase_over(ov, pi)) {
                    content += 1;
                    self.push_phrase(ov, &mut out, pi, if high { None } else { target }, neg, high);
                    out.lexical.extend(toks[i..i + n].iter().cloned());
                    target = None;
                    neg = false;
                    high = false;
                    i += n;
                    continue;
                }
            }
            if i + 1 < toks.len() && Grammar::has(&g.two_word_diminishers, &format!("{} {}", w, toks[i + 1])) {
                target = Some(LOW_TARGET);
                i += 2;
                continue;
            }
            // A negator or filler word that starts a known term is part of
            // it (`not bad`, `on my way`, `its fine`); one with nothing after
            // it to modify (`no`, `what`) is a word in its own right when
            // the data knows it. Intensifiers and diminishers always modify:
            // `very angry` is angry, read as intense, not the tag phrase.
            let known = |s: &str| self.real_term(s).is_some() || self.phrase_find(ov, s).is_some();
            let starts_term = (Grammar::has(&g.negators, w) || Grammar::has(&g.stopwords, w))
                && (2..=4).any(|m| i + m <= toks.len() && known(&toks[i..i + m].join(" ")));
            let last_word = i + 1 == toks.len()
                && (Grammar::has(&g.negators, w) || Grammar::has(&g.stopwords, w) || Grammar::has(&g.single_words, w) || Grammar::has(&g.pair_words, w))
                && known(w);
            let as_word = starts_term || last_word;
            if Grammar::has(&g.lewd, w) {
                out.lewd = true;
            } else if w == "lenny" {
                out.lenny = true;
            } else if w == "cute" {
                out.cute = true;
                out.cute_typed = true;
            } else if !as_word && Grammar::has(&g.single_words, w) {
                out.want_single = true;
            } else if !as_word && Grammar::has(&g.pair_words, w) {
                out.want_pair = true;
            } else if !as_word && Grammar::has(&g.intensifiers, w) {
                high = true;
            } else if !as_word && Grammar::has(&g.diminishers, w) {
                target = Some(LOW_TARGET);
            } else if !as_word && Grammar::has(&g.negators, w) {
                neg = true;
            } else if !as_word && Grammar::has(&g.stopwords, w) {
            } else {
                // longest vocab match, up to four words
                let mut hit = None;
                for n in (1..=4usize).rev() {
                    if i + n > toks.len() {
                        continue;
                    }
                    let phrase = toks[i..i + n].join(" ");
                    if let Some(ti) = self.real_term(&phrase) {
                        hit = Some((phrase, ti, n));
                        break;
                    }
                }
                content += 1;
                // The generated lexicon: longest phrase here. It takes over
                // when it is longer than the vocab match, or the same length
                // and either multi-word or a word the vocab only guesses at
                // through e5; words the data knows well keep their lists.
                let mut ph = None;
                for n in (1..=self.phrase_max(ov)).rev() {
                    if i + n > toks.len() {
                        continue;
                    }
                    if let Some(pi) = self.phrase_find(ov, &toks[i..i + n].join(" ")) {
                        ph = Some((pi, n));
                        break;
                    }
                }
                if let Some((pi, n)) = ph {
                    let vn = hit.as_ref().map(|h| h.2).unwrap_or(0);
                    let weak = hit.as_ref().is_none_or(|h| self.shadowed(h.1));
                    let over = self.phrase_over(ov, pi);
                    // A misspelling listed under a phrase whose key is a
                    // well-known vocab word (`tenitive` under `tentative`,
                    // `sleppy` under `sleepy`) reads as that word, corrected,
                    // so the spelling does not change the reading or lose
                    // the word's pinned faces.
                    let mut inherited = false;
                    if n == 1 && hit.is_none() && ctx.correct && !over {
                        if let Some(key) = self.phrase_view(ov, pi).map(|p| p.key).filter(|k| *k != w && !k.contains(' ')) {
                            if let Some(ti) = self.st.term_index(key) {
                                let (wc, kc): (Vec<char>, Vec<char>) = (w.chars().collect(), key.chars().collect());
                                if !self.shadowed(ti) && crate::text::damerau(&wc, &kc, 2) <= 2 {
                                    hit = Some((key.to_string(), ti, 1));
                                    out.corrected = Some(key.to_string());
                                    inherited = true;
                                }
                            }
                        }
                    }
                    if !inherited && (n > vn || (n == vn && (n > 1 || weak || over))) {
                        self.push_phrase(ov, &mut out, pi, if high { None } else { target }, neg, high);
                        out.lexical.extend(toks[i..i + n].iter().cloned());
                        target = None;
                        neg = false;
                        high = false;
                        i += n;
                        continue;
                    }
                }
                let mut adverb = false;
                if hit.is_none() && w.is_ascii() && w.ends_with("ly") && w.len() > 4 {
                    // `tentatively proud`: the adverb colours the head word
                    let stems = [
                        w[..w.len() - 2].to_string(),
                        format!("{}y", &w[..w.len() - 3]),
                        format!("{}e", &w[..w.len() - 1]),
                    ];
                    for stem in stems {
                        if let Some(ti) = self.st.term_index(&stem) {
                            hit = Some((stem, ti, 1));
                            adverb = true;
                            break;
                        }
                    }
                }
                if hit.is_none() && ctx.correct {
                    if let Some((k, ti)) = self.stem(w).and_then(|k| Some((k, self.st.term_index(k)?))) {
                        hit = Some((k.to_string(), ti, 1));
                        out.corrected = Some(k.to_string());
                    }
                }
                // A stretched word (`noooooooo`, `yesss`, `ughhhhh`) is its
                // plain form, as a vocab term or a phrase.
                let mut as_phrase: Option<(PRef, String)> = None;
                if hit.is_none() && ctx.correct {
                    if let Some(forms) = unstretch(w) {
                        for f in forms.iter().filter(|f| f.chars().count() >= 2) {
                            if let Some(ti) = self.st.term_index(f) {
                                hit = Some((f.clone(), ti, 1));
                                out.corrected = Some(f.clone());
                                break;
                            }
                            if let Some(pi) = self.phrase_find(ov, f) {
                                as_phrase = Some((pi, f.clone()));
                                break;
                            }
                        }
                    }
                }
                // Spelling correction: a lone word within 1-3 edits; inside
                // a longer query only a word of five letters or more within
                // one edit (`tentitive proude`, `happpy sad`), so `i feel
                // liek` is not rewritten.
                if hit.is_none() && as_phrase.is_none() && ctx.correct && (toks.len() == 1 || w.chars().count() >= 5) {
                    if let Some(k) = self.correct(ov, w, if toks.len() == 1 { None } else { Some(1) }) {
                        match self.st.term_index(k) {
                            Some(ti) => {
                                hit = Some((k.to_string(), ti, 1));
                                out.corrected = Some(k.to_string());
                            }
                            None => {
                                if let Some(pi) = self.phrase_find(ov, k) {
                                    as_phrase = Some((pi, k.to_string()));
                                }
                            }
                        }
                    }
                }
                if let Some((pi, k)) = as_phrase {
                    out.corrected = Some(k.clone());
                    self.push_phrase(ov, &mut out, pi, if high { None } else { target }, neg, high);
                    out.lexical.push(k);
                    target = None;
                    neg = false;
                    high = false;
                    i += 1;
                    continue;
                }
                out.lexical.push(w.to_string());
                if hit.is_none() && i + 1 == toks.len() {
                    out.trailing_unknown = true;
                }
                if let Some((name, ti, n)) = hit {
                    let mut q = self.qterm(&name, ti);
                    q.target = if high { None } else { target };
                    // `not` negates feelings (`not happy`), not things: `not
                    // without you` is not the opposite of `with`
                    q.neg = neg && self.st.term(ti).tier == 2;
                    q.high = high;
                    if adverb {
                        q.weight = 0.5;
                    }
                    if n > 1 {
                        out.lexical.push(name.clone());
                    }
                    out.terms.push(q);
                    target = None;
                    neg = false;
                    high = false;
                    i += n;
                    continue;
                }
            }
            i += 1;
        }
        match ctx.figures {
            Figures::Single => out.want_single = true,
            Figures::Pair => out.want_pair = true,
            Figures::Auto | Figures::Any => {}
        }
        out.sentence = content > 3;
        if out.terms.is_empty() && out.want_pair {
            let mut p = [0f32; N_EMO];
            for (e, v) in [("love", 0.5f32), ("happy", 0.3), ("calm", 0.2)] {
                if let Some(i) = self.emotions.iter().position(|x| x == e) {
                    p[i] = v;
                }
            }
            out.terms.push(QTerm {
                dense: self.st.term_index("together"),
                pair: Some(true),
                canon: Some("together".into()),
                ..QTerm::plain("together".into(), p)
            });
        }
        // A matched situation carries the meaning; leftover words only colour
        // it (`enjoy` in the tsundere line must not demand love).
        // (A phrase with no vocab entry counts too: that is how the
        // pre-library engine read it, and the weights were tuned on that.)
        if out.terms.iter().any(|t| t.dense.is_none()) {
            for t in out.terms.iter_mut().filter(|t| t.dense.is_some() && !t.dense_only) {
                t.weight = t.weight.min(0.5);
            }
        }
        match ctx.intensity {
            Intensity::Auto => {}
            Intensity::Low => {
                for t in out.terms.iter_mut().filter(|t| !t.dense_only && !t.high && !t.neg && t.target.is_none()) {
                    t.target = Some(LOW_TARGET);
                }
            }
            Intensity::High => {
                for t in out.terms.iter_mut().filter(|t| !t.dense_only) {
                    t.high = true;
                    t.target = None;
                }
            }
        }
        if let Some((p, t)) = ctx.target {
            out.terms.push(QTerm { per: Some(t), ..QTerm::plain("emotions".into(), p) });
        }
        if out.terms.len() == 1 && out.terms[0].dense.is_some() && !out.terms[0].dense_only {
            out.boost_term = Some(out.terms[0].name.clone());
        }
        let mains: Vec<&QTerm> = out.terms.iter().filter(|t| !t.dense_only).collect();
        if mains.is_empty() && out.lenny && toks.len() == 1 {
            out.canon_key = Some("lenny".into());
        }
        let mut key_user = false;
        if mains.len() == 1 && !out.want_single && !out.want_pair && !out.lewd {
            let t = mains[0];
            // the whole query first (`nervous laugh`), else the concept
            let whole = tokens(text).join(" ");
            let whole_has = self.has_canonical(ov, &whole);
            // A modifier the user typed (`a bit sad`, `very angry`) unpins
            // the bare concept; a phrase's own level (`nooo`, `okie`) keeps
            // its pins. `cute` keeps them too: `sleep cute` without the
            // sleep pins lost the face that was picked.
            let plain = (t.target.is_none() && !t.high) || t.soft_level;
            if plain && !t.neg && t.weight >= 1.0 {
                if whole_has {
                    key_user = if t.canon.as_deref() == Some(whole.as_str()) { t.user } else { !self.st.has_canonical(&whole) };
                    out.canon_key = Some(whole);
                } else {
                    key_user = t.user;
                    out.canon_key = t.canon.clone();
                }
            }
        }
        out.term_key = out.canon_key.clone().or_else(|| if mains.len() == 1 { mains[0].canon.clone() } else { None });
        out.term_user = if out.canon_key.is_some() { key_user } else { mains.len() == 1 && mains[0].user };
        out
    }

    /// Prefix completions for a partial word: words with canonical faces
    /// first, then emotion words, then lexicon keys, then by how many faces
    /// carry the tag. Tag counts alone put `idol` above `idk`.
    /// The user's own single-word spellings come first.
    pub(crate) fn completions<'a>(&'a self, ov: &'a OverlayTables, p: &str, k: usize) -> Vec<&'a str> {
        let mut c: Vec<(&str, usize)> = self
            .st
            .keys_from(p)
            .take_while(|(x, _)| x.starts_with(p))
            .filter(|(x, _)| !x.contains(' '))
            .collect();
        c.sort_by_key(|&(x, ti)| {
            let t = self.st.term(ti);
            std::cmp::Reverse((self.has_canonical(ov, x), t.tier, t.n))
        });
        let mut out: Vec<&str> = Vec::new();
        if !ov.spell.is_empty() {
            out.extend(ov.spellings().into_iter().filter(|s| s.starts_with(p) && !s.contains(' ')));
        }
        for (x, _) in c {
            if !out.contains(&x) {
                out.push(x);
            }
        }
        out.truncate(k);
        out
    }

    /// Rank-decayed credit from a precomputed neighbour list (e5's or the
    /// affect engine's), averaged over the query's terms; one slot per face.
    fn dense_scores<L: Iterator<Item = (usize, u32)>>(&self, terms: &[QTerm], list: impl Fn(usize) -> L) -> Vec<f32> {
        let mut out = vec![0f32; self.st.n_faces];
        let with: Vec<(&QTerm, usize)> = terms.iter().filter(|t| !t.neg).filter_map(|t| Some((t, t.dense?))).collect();
        for (t, d) in &with {
            for (rank, fi) in list(*d) {
                if let Some(o) = out.get_mut(fi as usize) {
                    *o += t.weight / (1.0 + rank as f32 / 10.0);
                }
            }
        }
        let n: f32 = with.iter().map(|(t, _)| t.weight).sum();
        if n > 0.0 {
            for v in out.iter_mut().filter(|v| **v != 0.0) {
                *v /= n;
            }
        }
        out
    }

    /// Best tier credit of any query word among each face's tags.
    fn lexical_scores(&self, words: &[String]) -> Vec<f32> {
        const TIER: [f32; 3] = [1.0, 0.8, 0.5];
        let mut out = vec![0f32; self.st.n_faces];
        let words: HashSet<&String> = words.iter().filter(|w| w.chars().count() >= 2).collect();
        for w in words {
            for (fi, tier) in self.st.postings(w) {
                if let Some(e) = out.get_mut(fi as usize) {
                    *e = e.max(TIER[usize::from(tier.min(2))]);
                }
            }
        }
        out
    }

    /// The hard filters: false when the options remove this face.
    fn admissible(&self, i: usize, f: &FaceNum, ctx: &Ctx) -> bool {
        if !ctx.filters {
            return true;
        }
        let o = i as u32;
        !(ctx.blocked.contains(&o)
            || ctx.exclude.contains(&o)
            || (ctx.safety == Safety::Strict && f.suggestive >= self.sexual_at)
            || (ctx.lenny == StyleMode::Hide && f.lenny >= 0.5)
            || (ctx.crude == StyleMode::Hide && f.crude >= self.crude_at)
            || (ctx.long == StyleMode::Hide && f.len > ctx.long_at)
            || ctx.max_len.is_some_and(|m| f.len > m)
            || (ctx.faces_only && f.face < 0.5)
            || ctx.min_quality.is_some_and(|q| f.quality < q)
            || ctx.emo_min.iter().any(|&(e, v)| f.r[e] < v)
            || ctx.emo_max.iter().any(|&(e, v)| f.r[e] > v))
    }

    fn prepare(&self, q: &Parsed, ctx: &Ctx) -> Prep {
        let mut canon = vec![0f32; self.st.n_faces];
        if ctx.pinned {
            if let Some(list) = q.canon_key.as_ref().and_then(|k| self.canonical(ctx.ov, k)) {
                for (fi, rank, _) in list {
                    if let (Some(c), Some(&b)) = (canon.get_mut(fi as usize), CANON_BONUS.get(rank)) {
                        *c = b;
                    }
                }
            }
        }
        let dense = self.dense_scores(&q.terms, |t| self.st.dense(t));
        let engine = self.dense_scores(&q.terms, |t| self.st.engine(t));
        let lex = self.lexical_scores(&q.lexical);
        let main: Vec<usize> = (0..q.terms.len()).filter(|&k| q.terms[k].weight >= 1.0 && !q.terms[k].dense_only).collect();
        let adv: Vec<usize> = (0..q.terms.len()).filter(|&k| q.terms[k].weight < 1.0 && !q.terms[k].dense_only).collect();
        let high = q.terms.iter().any(|t| t.high);
        // The neighbour lists were computed per single term, so they know
        // `sad` but not `a bit sad`, `shy proud` or `kissing face`. When the
        // query has structure the emotion match leads; for a plain word the
        // lists do (that is what the weights were tuned on).
        let structured = main.len() >= 2
            || !adv.is_empty()
            || q.terms.iter().any(|t| t.target.is_some() || t.high || t.neg || t.dense.is_none())
            || q.want_single
            || q.want_pair
            || q.lewd
            || q.lenny;
        let w = if structured {
            Weights { emo: ctx.w.emo * 2.0, dense: ctx.w.dense * 0.4, engine: ctx.w.engine * 0.4, lex: ctx.w.lex }
        } else {
            ctx.w
        };

        // which way the figures should go
        let pair_term = q.terms.iter().find_map(|t| t.pair);
        let want_pair = q.want_pair || pair_term == Some(true);
        let want_single = q.want_single || pair_term == Some(false);
        let ms: Vec<f32> = q.terms.iter().filter_map(|t| t.m).collect();
        let pref = if ms.is_empty() {
            0.0
        } else {
            ((ms.iter().sum::<f32>() / ms.len() as f32 - 0.3) / 0.3).clamp(-1.0, 1.0)
        };

        let mut usage = HashMap::new();
        if ctx.usage_lift > 0.0 && !ctx.usage.is_empty() {
            let term = q.term_key.as_deref();
            let by_term = term.and_then(|k| ctx.usage.by_term.get(k));
            // distinct ids in id order; an old id (ALIA) and its new one
            // land on the same face and add up
            let ids: std::collections::BTreeSet<&FaceId> = ctx.usage.global.keys().chain(by_term.into_iter().flat_map(|m| m.keys())).collect();
            for id in ids {
                if let Some(o) = self.st.by_fid(*id) {
                    let has_term = by_term.is_some_and(|m| m.get(id).is_some_and(|v| *v > 0.0));
                    let w = ctx.usage.weight(*id, term);
                    usage
                        .entry(o)
                        .and_modify(|e: &mut (f32, bool)| {
                            e.0 = (e.0 + w).min(1.0);
                            e.1 |= has_term;
                        })
                        .or_insert((w, has_term));
                }
            }
        }
        let mut boosts: Option<HashMap<FaceId, f32>> =
            q.boost_term.as_ref().map(|b| self.st.boosts(b)).filter(|v| !v.is_empty()).map(|v| v.into_iter().collect());
        // the user's boosts win per face; they apply to the query's concept
        if !ctx.ov.boosts.is_empty() {
            let keys = [q.boost_term.as_ref(), q.term_key.as_ref().filter(|k| Some(*k) != q.boost_term.as_ref())];
            for m in keys.into_iter().flatten().filter_map(|k| ctx.ov.boosts.get(k)) {
                boosts.get_or_insert_with(HashMap::new).extend(m.iter().map(|(id, b)| (*id, *b)));
            }
        }
        Prep { canon, dense, engine, lex, main, adv, high, w, want_pair, want_single, pref, usage, boosts }
    }

    fn term_score(t: &QTerm, f: &FaceNum) -> f32 {
        if let Some(tv) = &t.per {
            return t
                .p
                .iter()
                .zip(&f.r)
                .zip(tv)
                .map(|((p, r), tg)| if *r < MIN_PRESENT { 0.0 } else { p * (1.0 - (r - tg).abs()) })
                .sum();
        }
        if t.neg {
            return 1.0 - dot(&t.p, &f.r);
        }
        match t.target {
            None => dot(&t.p, &f.r),
            Some(tg) => t
                .p
                .iter()
                .zip(&f.r)
                .map(|(p, r)| if *r < MIN_PRESENT { 0.0 } else { p * (1.0 - (r - tg).abs()) })
                .sum(),
        }
    }

    /// One face's score (None = not a candidate). The order of the
    /// additions is the pre-library engine's; keep it, or scores change in
    /// the last bit.
    fn face_score(&self, p: &Prep, q: &Parsed, ctx: &Ctx, i: usize, f: &FaceNum, why: Option<&mut HitWhy>) -> Option<f32> {
        if !self.admissible(i, f, ctx) {
            return None;
        }
        let (d, g, l) = (p.dense[i], p.engine[i], p.lex[i]);
        let mut e = 0.0f32;
        if !p.main.is_empty() {
            let main = p.main.iter().map(|&k| Self::term_score(&q.terms[k], f));
            e = if q.sentence { main.sum::<f32>() / p.main.len() as f32 } else { main.fold(f32::INFINITY, f32::min) };
        }
        for &k in &p.adv {
            let t = &q.terms[k];
            e += W_ADVERB * t.weight * Self::term_score(t, f);
        }
        let c = p.canon[i];
        let (u, used_for_term) = if p.usage.is_empty() { (0.0, false) } else { p.usage.get(&(i as u32)).copied().unwrap_or((0.0, false)) };
        if c == 0.0
            && e < 0.05
            && d == 0.0
            && g == 0.0
            && l == 0.0
            && !used_for_term
            && !(q.terms.is_empty() && (q.lenny || q.lewd || q.cute))
        {
            return None;
        }
        let w = p.w;
        let base = c + w.emo * e + w.dense * d + w.engine * g + w.lex * l.min(1.0);
        let mut s = base;
        if p.high {
            s += 0.15 * f.intensity;
        }
        if ctx.figures != Figures::Any {
            let multi = f.multi > 0.5;
            if p.want_single {
                s += if multi { -0.35 } else { 0.05 };
            } else if p.want_pair {
                s += 0.3 * (2.0 * f.multi - 1.0);
            } else if p.pref > 0.0 {
                s += 0.15 * p.pref * (2.0 * f.multi - 1.0);
            } else if multi {
                // plain emotions read best on one face
                s -= 0.08 - 0.07 * p.pref;
            }
        }
        s += W_QUALITY * ((f.quality - 5.0) / 3.0).clamp(-1.0, 1.0);
        if q.cute {
            s += W_CUTE * f.cute;
        }
        if let Some(b) = p.boosts.as_ref().and_then(|m| m.get(&self.st.fid(i))) {
            s += W_BOOST * *b;
        }
        s -= 0.4 * (1.0 - f.face).max(0.0);
        if ctx.long != StyleMode::Allow {
            s -= (0.012 * f.len.saturating_sub(ctx.long_at) as f32).min(0.2);
        }
        // Under strict, asking never helps: no bonus, the penalty stays.
        let asked = q.lewd && ctx.safety != Safety::Strict;
        if f.suggestive >= self.innuendo_at {
            let v = 0.05 * 4f32.powf(f.suggestive - 1.0);
            if asked {
                s += 2.0 * v;
            } else if ctx.safety != Safety::Off {
                s -= v;
            }
        }
        if q.lenny {
            s += 0.4 * f.lenny;
        } else if ctx.lenny != StyleMode::Allow {
            s -= 0.15 * f.lenny;
        }
        if f.crude >= self.crude_at && !asked && ctx.crude != StyleMode::Allow {
            s -= 0.1;
        }
        let adjust = s - base;
        if u > 0.0 {
            s += ctx.usage_lift * u;
        }
        let mut jitter = 0.0;
        if ctx.variety > 0.0 && c == 0.0 {
            jitter = ctx.variety * JITTER * (jitter_unit(ctx.seed, self.st.fid(i)) - 0.5);
            s += jitter;
        }
        if let Some(wy) = why {
            *wy = HitWhy { canon: c, emotion: e, dense: d, engine: g, lexical: l, adjust, usage: ctx.usage_lift * u, jitter, total: s };
        }
        Some(s)
    }

    /// Scores for every face (None = not a candidate).
    fn score_all(&self, q: &Parsed, ctx: &Ctx) -> (Vec<Option<f32>>, Prep) {
        let p = self.prepare(q, ctx);
        if ctx.impossible {
            return (vec![None; self.st.n_faces], p);
        }
        let v = self.st.nums().iter().enumerate().map(|(i, f)| self.face_score(&p, q, ctx, i, &f, None)).collect();
        (v, p)
    }

    /// Best first (score, then FaceId), thinned of near-duplicates, after
    /// `offset`. Only the best few thousand candidates are sorted; the rest
    /// only if thinning used those up.
    fn rank(&self, scores: Vec<Option<f32>>, ctx: &Ctx) -> Vec<(usize, f32)> {
        let want = ctx.limit + ctx.offset;
        let mut v: Vec<(usize, f32)> = scores.into_iter().enumerate().filter_map(|(i, s)| s.map(|s| (i, s))).collect();
        let order = |a: &(usize, f32), b: &(usize, f32)| {
            b.1.total_cmp(&a.1).then_with(|| {
                if ctx.legacy_ties {
                    a.0.cmp(&b.0)
                } else {
                    self.st.fid(a.0).cmp(&self.st.fid(b.0))
                }
            })
        };
        let head = (want * 20).max(1);
        if v.len() > head {
            v.select_nth_unstable_by(head, order);
        }
        let split = head.min(v.len());
        v[..split].sort_by(order);
        let mut out: Vec<(usize, f32)> = Vec::with_capacity(want);
        // the char sets of `out`, kept beside it (they may be derived from
        // the text, so each is made once)
        let mut out_chars: Vec<std::borrow::Cow<'_, [u32]>> = Vec::with_capacity(want);
        // jaccard is at most 1: a threshold of 1 or more never drops a face
        let thin = ctx.dedupe < 1.0;
        let mut i = 0;
        while i < v.len() && out.len() < want {
            if i == split {
                v[split..].sort_by(order);
            }
            let (fi, s) = v[i];
            i += 1;
            if thin {
                let c = self.st.chars(fi);
                if out_chars.iter().any(|p| jaccard(&c, p) > ctx.dedupe) {
                    continue;
                }
                out_chars.push(c);
            }
            out.push((fi, s));
        }
        out.drain(..ctx.offset.min(out.len()));
        out
    }

    /// Faces containing pasted glyph text, shortest first (then by id).
    /// Strings the data does not consider faces (URLs, bios) never lead a
    /// substring search.
    fn glyph_text(&self, needle: &str, ctx: &Ctx, limit: usize) -> Vec<(usize, f32)> {
        let nums = self.st.nums();
        let mut hits: Vec<(u32, u64, usize)> = (0..self.st.n_faces)
            .filter(|&i| {
                let f = nums.at(i);
                f.face >= 0.5 && self.st.text(i).contains(needle) && self.admissible(i, &f, ctx)
            })
            .map(|i| (nums.at(i).len, if ctx.legacy_ties { 0 } else { self.st.fid(i).as_u64() }, i))
            .collect();
        hits.sort();
        hits.into_iter().take(limit).enumerate().map(|(r, (_, _, i))| (i, 1.0 - r as f32 * 0.001)).collect()
    }

    /// The query is a partial word to complete: its completions.
    fn partial<'a>(&'a self, q: &Parsed, toks: &[String], ctx: &Ctx<'a>) -> Option<Vec<&'a str>> {
        if ctx.complete_partial
            && q.terms.is_empty()
            && q.canon_key.is_none()
            && toks.len() == 1
            && self.real_term(&toks[0]).is_none()
        {
            let comps = self.completions(ctx.ov, &toks[0], 4);
            return (!comps.is_empty()).then_some(comps);
        }
        None
    }

    /// The query ends in a partial word after words that were read (`very
    /// sa`, `im so ti`, `it was just o`): (the words before it, its
    /// completions), for search-as-you-type.
    fn trailing<'a>(&'a self, q: &Parsed, toks: &[String], ctx: &Ctx<'a>) -> Option<(String, Vec<&'a str>)> {
        if !ctx.complete_partial || toks.len() < 2 || !q.trailing_unknown {
            return None;
        }
        let last = toks.last()?;
        let comps = self.completions(ctx.ov, last, 4);
        (!comps.is_empty()).then(|| (toks[..toks.len() - 1].join(" "), comps))
    }

    /// Search several readings of one query (a partial word's completions)
    /// and keep each face's best score, a little less for later readings.
    fn blend(&self, queries: &[String], ctx: &Ctx) -> Found {
        let mut best: Vec<Option<f32>> = vec![None; self.st.n_faces];
        let mut pinned = Vec::new();
        for (rank, c) in queries.iter().enumerate() {
            let mut qc = self.parse(c, ctx);
            if !c.contains(' ') {
                qc.boost_term = Some(c.to_string()).filter(|c| self.st.has_boosts(c) || ctx.ov.boosts.contains_key(c));
            }
            pinned.extend(self.pinned_set(qc.canon_key.as_ref(), ctx));
            let (sc, _) = self.score_all(&qc, ctx);
            for (b, s) in best.iter_mut().zip(sc) {
                if let Some(s) = s {
                    let s = s - 0.25 * rank as f32;
                    *b = Some(b.map_or(s, |x: f32| x.max(s)));
                }
            }
        }
        let hits: Vec<(usize, f32, bool, bool)> = self.rank(best, ctx).into_iter().map(|(i, s)| Self::pin_flags(i, s, &pinned)).collect();
        let why = if ctx.full { vec![None; hits.len()] } else { Vec::new() };
        Found { hits, corrected: None, term_key: None, term_user: false, why }
    }

    /// (ordinal, pinned by the user) of the faces pinned for `key`.
    fn pinned_set(&self, key: Option<&String>, ctx: &Ctx) -> Vec<(u32, bool)> {
        if !ctx.pinned {
            return Vec::new();
        }
        key.and_then(|k| self.canonical(ctx.ov, k)).map(|l| l.iter().map(|&(fi, _, u)| (fi, u)).collect()).unwrap_or_default()
    }

    /// Search a non-empty query.
    pub(crate) fn search(&self, text: &str, ctx: &Ctx) -> Found {
        let mut found = Found { hits: Vec::new(), corrected: None, term_key: None, term_user: false, why: Vec::new() };
        let t = text.trim();
        if t.is_empty() && ctx.target.is_none() {
            return found;
        }
        // A typed emoticon the grammar knows (`:/`, `T_T`, `xD`, with or
        // without a `!`) is the concept it stands for, reported as a
        // correction.
        if let Some(concept) = self.grammar.emoticon(t.trim_end_matches(['!', '?', '.', ','])).filter(|c| *c != t) {
            let mut f = self.search(concept, ctx);
            f.corrected = Some(concept.to_string());
            return f;
        }
        if ctx.glyph && !t.is_empty() && is_glyphy(t) {
            let mut hits = self.glyph_text(t, ctx, ctx.limit + ctx.offset);
            if !hits.is_empty() {
                hits.drain(..ctx.offset.min(hits.len()));
                found.hits = hits.into_iter().map(|(i, s)| (i, s, false, false)).collect();
                return found;
            }
        }
        let q = self.parse(t, ctx);
        let toks = tokens(t);
        // A partial word (`sa`, `hu`, `embar`): blend its likeliest completions.
        if let Some(comps) = self.partial(&q, &toks, ctx) {
            return self.blend(&comps.iter().map(|c| c.to_string()).collect::<Vec<_>>(), ctx);
        }
        // Words then a partial word (`very sa`): the same, keeping the words.
        if let Some((head, comps)) = self.trailing(&q, &toks, ctx) {
            let mut f = self.blend(&comps.iter().map(|c| format!("{head} {c}")).collect::<Vec<_>>(), ctx);
            f.corrected = q.corrected.clone();
            return f;
        }
        let (scores, prep) = self.score_all(&q, ctx);
        let pinned = self.pinned_set(q.canon_key.as_ref(), ctx);
        found.hits = self.rank(scores, ctx).into_iter().map(|(i, s)| Self::pin_flags(i, s, &pinned)).collect();
        if ctx.full {
            let nums = self.st.nums();
            found.why = found
                .hits
                .iter()
                .map(|&(i, _, _, _)| {
                    let mut w = HitWhy::default();
                    self.face_score(&prep, &q, ctx, i, &nums.at(i), Some(&mut w)).map(|_| w)
                })
                .collect();
        }
        found.corrected = q.corrected.clone();
        found.term_key = q.term_key.clone();
        found.term_user = q.term_user;
        found
    }

    fn pin_flags(i: usize, s: f32, pinned: &[(u32, bool)]) -> (usize, f32, bool, bool) {
        match pinned.iter().find(|p| p.0 == i as u32) {
            Some(&(_, user)) => (i, s, true, user),
            None => (i, s, false, false),
        }
    }

    /// How a query was read, short enough for one line under a search box:
    /// `very angry (angry) + shy (shy, scared) · pair · pinned`. Follows the
    /// same branches as search(), so it describes what produced the results.
    pub(crate) fn reading(&self, text: &str, ctx: &Ctx) -> Reading {
        let mut r = self.reading_line(text, ctx);
        if ctx.full {
            r.debug = Some(self.explain(text.trim(), ctx));
        }
        r
    }

    fn reading_line(&self, text: &str, ctx: &Ctx) -> Reading {
        let t = text.trim();
        let mut r = Reading::default();
        if t.is_empty() && ctx.target.is_none() {
            return r;
        }
        if let Some(concept) = self.grammar.emoticon(t.trim_end_matches(['!', '?', '.', ','])).filter(|c| *c != t) {
            let mut r = self.reading_line(concept, ctx);
            r.line = format!("\u{201c}{t}\u{201d} as {}", r.line);
            return r;
        }
        if ctx.glyph && !t.is_empty() && is_glyphy(t) && !self.glyph_text(t, ctx, 1).is_empty() {
            r.line = format!("faces containing \u{201c}{t}\u{201d}");
            r.mode = ReadMode::Glyph;
            return r;
        }
        let q = self.parse(t, ctx);
        let toks = tokens(t);
        if let Some(comps) = self.partial(&q, &toks, ctx) {
            r.line = format!("completing: {}", comps.join(", "));
            r.mode = ReadMode::Partial { completions: comps.iter().map(|c| c.to_string()).collect() };
            return r;
        }
        if let Some((head, comps)) = self.trailing(&q, &toks, ctx) {
            let mut r = self.reading_line(&head, ctx);
            let completions: Vec<String> = comps.iter().map(|c| c.to_string()).collect();
            // the blend pins per completed query, not for the head alone
            let head_line = r.line.replace(" \u{b7} your pinned faces first", "").replace(" \u{b7} pinned faces first", "");
            r.line = if matches!(r.mode, ReadMode::Nothing | ReadMode::Tags | ReadMode::Empty) {
                format!("completing: {}", completions.join(", "))
            } else {
                format!("{head_line} + completing: {}", completions.join(", "))
            };
            r.mode = ReadMode::Partial { completions };
            return r;
        }
        let top = |t: &QTerm| -> Vec<String> {
            let mut v: Vec<(usize, f32)> = t.p.iter().cloned().enumerate().filter(|(_, x)| *x > 0.15).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v.iter().take(2).map(|(i, _)| self.emotions[*i].clone()).collect()
        };
        let mains: Vec<&QTerm> = q.terms.iter().filter(|t| !t.dense_only).collect();
        r.terms = mains
            .iter()
            .map(|t| ReadTerm {
                term: t.name.clone(),
                emotions: top(t),
                modifier: if t.high {
                    Some(Modifier::High)
                } else if t.target.is_some() {
                    Some(Modifier::Low)
                } else {
                    None
                },
                negated: t.neg,
                user: t.user,
            })
            .collect();
        let mut parts: Vec<String> = mains
            .iter()
            .map(|t| {
                let emo = top(t);
                format!(
                    "{}{}{}{}{}",
                    if t.neg { "not " } else { "" },
                    if t.high && !t.name.starts_with("very ") { "very " } else { "" },
                    if t.target.is_some() && !t.name.starts_with("a bit ") { "a bit " } else { "" },
                    t.name,
                    if emo.is_empty() { String::new() } else { format!(" ({})", emo.join(", ")) },
                )
            })
            .collect();
        let tags_only = parts.is_empty() && !q.lexical.is_empty();
        let mut out = if tags_only { format!("tags: {}", q.lexical.join(", ")) } else { std::mem::take(&mut parts).join(" + ") };
        let mut flags = Vec::new();
        if let Some(c) = &q.corrected {
            flags.push(format!("as \u{201c}{c}\u{201d}"));
        }
        if q.want_pair {
            flags.push("pair".to_string());
        }
        if q.want_single {
            flags.push("single".to_string());
        }
        let lewd = if ctx.safety == Safety::Strict { "suggestive (off: strict)" } else { "suggestive" };
        for (on, name) in [(q.cute, "cute"), (q.lenny, "lenny"), (q.lewd, lewd)] {
            if on {
                flags.push(name.to_string());
            }
        }
        if mains.iter().any(|t| t.user) {
            flags.push("yours".to_string());
        }
        if ctx.pinned && q.canon_key.as_ref().is_some_and(|k| self.has_canonical(ctx.ov, k)) {
            let user = q.canon_key.as_ref().and_then(|k| ctx.ov.canon.get(k)).is_some_and(|l| l.has_user());
            flags.push(if user { "your pinned faces first" } else { "pinned faces first" }.to_string());
        }
        if out.is_empty() && flags.is_empty() {
            r.line = "nothing recognised".to_string();
            r.mode = ReadMode::Nothing;
            return r;
        }
        r.mode = if tags_only {
            ReadMode::Tags
        } else if mains.iter().any(|t| t.situation) {
            ReadMode::Situation
        } else if q.sentence {
            ReadMode::Sentence
        } else if mains.len() >= 2 || mains.iter().any(|t| t.neg || t.high || t.target.is_some() || t.weight < 1.0) {
            ReadMode::Terms
        } else if mains.is_empty() && q.canon_key.is_none() {
            ReadMode::Nothing
        } else {
            ReadMode::Concept
        };
        for f in flags {
            if !out.is_empty() {
                out.push_str(" \u{b7} ");
            }
            out.push_str(&f);
        }
        r.line = out;
        r
    }

    pub(crate) fn admissible_pub(&self, i: usize, ctx: &Ctx) -> bool {
        self.st.nums().get(i).is_some_and(|f| self.admissible(i, &f, ctx))
    }

    /// Faces that feel like face `src`: cosine of the emotion profiles,
    /// less differences in figures, cuteness and suggestiveness, plus the
    /// usual quality term. Near-duplicates of `src` are left out.
    pub(crate) fn similar(&self, src: usize, ctx: &Ctx) -> Vec<(usize, f32)> {
        let nums = self.st.nums();
        let Some(a) = nums.get(src) else { return Vec::new() };
        let norm = |r: &[f32; N_EMO]| dot(r, r).sqrt();
        let na = norm(&a.r);
        let src_chars = self.st.chars(src);
        let thin = ctx.dedupe < 1.0;
        let scores = nums
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if i == src || !self.admissible(i, &f, ctx) || (thin && jaccard(&self.st.chars(i), &src_chars) > ctx.dedupe) {
                    return None;
                }
                let nb = norm(&f.r);
                if na == 0.0 || nb == 0.0 {
                    return None;
                }
                let mut s = dot(&a.r, &f.r) / (na * nb);
                s -= 0.2 * (a.multi - f.multi).abs() + 0.1 * (a.cute - f.cute).abs() + 0.1 * (a.suggestive - f.suggestive).abs();
                s += W_QUALITY * ((f.quality - 5.0) / 3.0).clamp(-1.0, 1.0);
                s -= 0.4 * (1.0 - f.face).max(0.0);
                if ctx.variety > 0.0 {
                    s += ctx.variety * JITTER * (jitter_unit(ctx.seed, self.st.fid(i)) - 0.5);
                }
                Some(s)
            })
            .collect();
        self.rank(scores, ctx)
    }

    /// For debugging: how a query was read.
    pub(crate) fn explain(&self, text: &str, ctx: &Ctx) -> String {
        let q = self.parse(text, ctx);
        let terms: Vec<String> = q
            .terms
            .iter()
            .map(|t| {
                let top: Vec<String> = {
                    let mut v: Vec<(usize, f32)> = t.p.iter().cloned().enumerate().filter(|(_, x)| *x > 0.05).collect();
                    v.sort_by(|a, b| b.1.total_cmp(&a.1));
                    v.iter().take(3).map(|(i, x)| format!("{} {:.2}", self.emotions[*i], x)).collect()
                };
                format!(
                    "{}{}{}{}{} [{}]{}",
                    if t.neg { "not " } else { "" },
                    if t.high { "very " } else { "" },
                    t.target.map(|x| format!("~{x} ")).unwrap_or_default(),
                    t.name,
                    if t.weight < 1.0 { " (adverb)" } else { "" },
                    top.join(", "),
                    t.pair.map(|p| if p { " pair" } else { " single" }).unwrap_or(""),
                )
            })
            .collect();
        format!(
            "terms: {} | words: {} | pair {} single {} lewd {} lenny {} sentence {} | canon {:?} -> {:?}",
            terms.join("; "),
            q.lexical.join(" "),
            q.want_pair,
            q.want_single,
            q.lewd,
            q.lenny,
            q.sentence,
            q.canon_key,
            q.canon_key.as_ref().and_then(|k| self.canonical(ctx.ov, k)).map(|v| v
                .iter()
                .map(|(fi, r, u)| format!("{}{}:{}", r, if *u { "*" } else { "" }, self.st.text(*fi as usize)))
                .collect::<Vec<_>>())
        )
    }
}
