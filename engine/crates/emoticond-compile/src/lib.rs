//! emoticond-compile: source data to a `.kmj` data file (format 3,
//! docs/format.md).
//!
//! [`Sources`] is the compiler's input, already parsed. [`legacy`] reads it
//! from today's private inputs (the `work/engine/` export, `data/*.jsonl`,
//! `situations.json`, `data/grammar/`, `data/licence/`); the documented
//! source bundle will be a second adapter onto
//! the same type. [`compile`] turns it into the file's bytes.
//!
//! The output is deterministic: the same sources give the same bytes, on
//! any machine. Nothing in it depends on time, hashing order or the
//! environment (`build_date` is whatever the caller passes, empty by
//! default).

pub mod legacy;
pub mod policy;

use emoticond::data::format::*;
use emoticond::{FaceId, Grammar};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// Build parameters recorded in the manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// `full`, `core` or `lite`.
    pub dataset: String,
    /// Calver of the data release, or `dev`.
    pub data_version: String,
    /// Free text, usually `YYYY-MM-DD`; empty keeps builds reproducible.
    pub build_date: String,
    /// `private`, or `public@<digest>` for a policy-checked build.
    pub policy: String,
    /// SPDX id of the data licence.
    pub licence: String,
    /// Faces predicted below this were left out by the export (recorded only).
    pub quality_cut: Option<f32>,
    pub innuendo_at: f32,
    pub sexual_at: f32,
    pub crude_at: f32,
    pub long_at_default: u16,
    /// How the big tables are stored.
    pub encoding: Encoding,
}

/// How a face's numbers are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumsEncoding {
    /// `FNUM`: 28 four-byte fields a face (format 3.0).
    F32,
    /// `FNQ2`: a 2-byte code per field into a per-field table of every
    /// distinct value. Lossless (bit for bit) while a field has at most
    /// 65,536 distinct values, which the compiler checks.
    Q16,
    /// `FNQ1`: a 1-byte code per field. Lossless for fields with at most
    /// 256 distinct values; others are quantised to at most 256 levels
    /// (uniform bins, each stored as the mean of its values) that never
    /// straddle a threshold the engine tests (safety, lenny, crude, multi,
    /// face).
    Q8,
}

/// How the neighbour lists (`dense`, `engine`) are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEncoding {
    /// `DNSE`/`ENGN`: `dense_k` u32 ordinals a term (format 3.0).
    U32,
    /// `DNSP`/`ENGP`: ordinals bit-packed at the width the set needs, empty
    /// slots at the end of a list not stored. Lossless.
    Packed,
}

/// How tag postings are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostingsEncoding {
    /// `POST`: one u32 a posting (format 3.0).
    U32,
    /// `PSTV`: varints of the ordinal gap and tier. Lossless.
    Varint,
}

/// Every storage choice. [`Encoding::legacy`] is format 3.0 exactly;
/// [`Encoding::compact`] is lossless and needs a 3.1 reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub nums: NumsEncoding,
    pub lists: ListEncoding,
    pub postings: PostingsEncoding,
    /// Store each face's char set (`FCHR`/`CHRS`); without them a 3.1
    /// reader derives it from the text when it needs it. Lossless.
    pub store_chars: bool,
    /// Store each distinct string once in `STRS` (spans may share bytes;
    /// any reader reads that). Lossless.
    pub dedupe_strings: bool,
}

impl Encoding {
    /// Format 3.0, byte for byte as the first compiler wrote it.
    pub const fn legacy() -> Encoding {
        Encoding {
            nums: NumsEncoding::F32,
            lists: ListEncoding::U32,
            postings: PostingsEncoding::U32,
            store_chars: true,
            dedupe_strings: false,
        }
    }
    /// Every lossless saving (format 3.1).
    pub const fn compact() -> Encoding {
        Encoding {
            nums: NumsEncoding::Q16,
            lists: ListEncoding::Packed,
            postings: PostingsEncoding::Varint,
            store_chars: false,
            dedupe_strings: true,
        }
    }
    /// `compact` with the face numbers quantised to one byte (lossy).
    pub const fn small() -> Encoding {
        Encoding { nums: NumsEncoding::Q8, ..Encoding::compact() }
    }
    /// By name: `legacy`, `compact`, `small`.
    pub fn named(name: &str) -> Option<Encoding> {
        match name {
            "legacy" | "3.0" => Some(Encoding::legacy()),
            "compact" | "lossless" => Some(Encoding::compact()),
            "small" | "q8" => Some(Encoding::small()),
            _ => None,
        }
    }
    /// Whether a 3.0 reader can read the result.
    pub fn is_3_0(&self) -> bool {
        self.nums == NumsEncoding::F32
            && self.lists == ListEncoding::U32
            && self.postings == PostingsEncoding::U32
            && self.store_chars
    }
    /// `nums=q16,lists=packed,postings=varint,chars=derived,strings=dedupe`
    pub fn describe(&self) -> String {
        format!(
            "nums={},lists={},postings={},chars={},strings={}",
            match self.nums {
                NumsEncoding::F32 => "f32",
                NumsEncoding::Q16 => "q16",
                NumsEncoding::Q8 => "q8",
            },
            match self.lists {
                ListEncoding::U32 => "u32",
                ListEncoding::Packed => "packed",
            },
            match self.postings {
                PostingsEncoding::U32 => "u32",
                PostingsEncoding::Varint => "varint",
            },
            if self.store_chars { "stored" } else { "derived" },
            if self.dedupe_strings { "dedupe" } else { "plain" },
        )
    }
}

impl Default for Encoding {
    fn default() -> Encoding {
        Encoding::compact()
    }
}

impl Default for Params {
    fn default() -> Params {
        Params {
            dataset: "full".into(),
            data_version: "dev".into(),
            build_date: String::new(),
            policy: "private".into(),
            licence: "CC-BY-4.0".into(),
            quality_cut: None,
            innuendo_at: 0.5,
            sexual_at: 1.5,
            crude_at: 0.5,
            long_at_default: 14,
            encoding: Encoding::default(),
        }
    }
}

/// One face, as the export gives it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaceIn {
    pub text: String,
    /// Emotion intensities, in `Sources::emotions` order.
    pub r: [f32; N_EMO],
    pub multi: f32,
    pub cute: f32,
    pub intensity: f32,
    pub suggestive: f32,
    pub lenny: f32,
    pub face: f32,
    pub quality: f32,
    /// 0..1 (the v1 crude flag gives 1.0).
    pub crude: f32,
    /// Tag words with their tier (0 best .. 2).
    pub words: Vec<(String, u8)>,
}

/// One vocab term (one line of the export's vocab, in order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TermIn {
    pub key: String,
    pub p: [f32; N_EMO],
    pub m: Option<f32>,
    pub n: u32,
    pub tier: u32,
    pub weak: bool,
}

/// A phrase of the generated search lexicon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhraseIn {
    pub key: String,
    pub p: [f32; N_EMO],
    pub level: Option<u8>,
    pub pair: Option<bool>,
    /// How it may be typed (raw text; tokenised by the compiler). The key
    /// itself is always a spelling too.
    pub spellings: Vec<String>,
    pub words: Vec<String>,
    pub cute: bool,
    pub lenny: bool,
    pub lewd: bool,
    /// Beats a vocab term of the same spelling however well the data knows
    /// the word (`"over": true`; hand-written entries).
    pub over: bool,
}

/// A hand-written situation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SituationIn {
    pub name: String,
    /// Raw spellings; tokenised by the compiler.
    pub matches: Vec<String>,
    pub p: [f32; N_EMO],
    pub pair: Option<bool>,
    pub words: Vec<String>,
}

/// One canonical pick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanonIn {
    pub term: String,
    pub text: String,
    pub rank: u64,
    /// Hand-picked (before the agent's picks).
    pub hand: bool,
}

/// A curated boost.
#[derive(Debug, Clone, PartialEq)]
pub struct BoostIn {
    pub term: String,
    pub id: FaceId,
    pub boost: f32,
}

/// An id that left the data: renamed (`new`), or retired with a reason
/// (`RETIRED_*` in `emoticond::data::format`).
#[derive(Debug, Clone, PartialEq)]
pub struct AliasIn {
    pub old: FaceId,
    pub new: Option<FaceId>,
    pub reason: u32,
}

/// Everything a data file is built from.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub params: Params,
    /// The 19 emotion names, in profile order.
    pub emotions: Vec<String>,
    pub extras: Vec<String>,
    /// Faces, in the export's order.
    pub faces: Vec<FaceIn>,
    /// Vocab terms; a key may repeat (the last wins for lookups).
    pub vocab: Vec<TermIn>,
    pub dense_k: usize,
    /// Per vocab term, `dense_k` faces as indices into `faces`; `NONE` = none.
    pub dense: Vec<u32>,
    /// The same from the affect engine, when the export has it.
    pub engine: Option<Vec<u32>>,
    pub phrases: Vec<PhraseIn>,
    pub situations: Vec<SituationIn>,
    /// Canonical picks, in any order.
    pub canonical: Vec<CanonIn>,
    /// Boosts; a later (term, id) replaces an earlier one.
    pub boosts: Vec<BoostIn>,
    /// One per language.
    pub grammar: Vec<Grammar>,
    pub licence: String,
    pub attribution: String,
    /// Digest of the inputs (recorded as `source_rev`).
    pub source_rev: String,
    /// Ids renamed or retired since earlier releases (`ALIA`, `RETD`).
    pub aliases: Vec<AliasIn>,
}

impl Sources {
    /// Keep only the faces `keep` accepts (a data set: `core`, `lite`).
    /// Neighbour-list slots that pointed at a dropped face become `NONE` in
    /// place, so every other neighbour keeps its rank; canonical picks of
    /// dropped faces fall away at compile time; boosts are keyed by id and
    /// stay. Returns how many faces were kept.
    pub fn retain_faces(&mut self, mut keep: impl FnMut(&FaceIn) -> bool) -> usize {
        let mut new_index = vec![NONE; self.faces.len()];
        let mut kept = 0u32;
        let faces = std::mem::take(&mut self.faces);
        for (i, f) in faces.into_iter().enumerate() {
            if keep(&f) {
                new_index[i] = kept;
                kept += 1;
                self.faces.push(f);
            }
        }
        let remap = |l: &mut Vec<u32>| l.iter_mut().for_each(|x| *x = new_index.get(*x as usize).copied().unwrap_or(NONE));
        remap(&mut self.dense);
        if let Some(e) = self.engine.as_mut() {
            remap(e);
        }
        kept as usize
    }

    /// Cut every neighbour list to its first `k` slots (`lite`).
    pub fn truncate_neighbours(&mut self, k: usize) {
        let old = self.dense_k;
        if k >= old {
            return;
        }
        self.dense = cut_lists(&self.dense, old, k);
        self.engine = self.engine.as_ref().map(|e| cut_lists(e, old, k));
        self.dense_k = k;
    }
}

/// Why compiling failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CompileError {
    #[error("{0}")]
    Invalid(String),
    /// A public build met an input the policy does not allow.
    #[error("{0}")]
    Policy(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn invalid(m: impl Into<String>) -> CompileError {
    CompileError::Invalid(m.into())
}

/// The first `k` of every `old`-long list.
fn cut_lists(l: &[u32], old: usize, k: usize) -> Vec<u32> {
    l.chunks(old.max(1)).flat_map(|c| c[..k.min(c.len())].iter().copied()).collect()
}

/// `ALIA` and `RETD` records.
type AliasTables = (Vec<[u64; 2]>, Vec<[u64; 2]>);

/// The string heap and its spans.
#[derive(Default)]
struct Heap {
    bytes: Vec<u8>,
    /// with `dedupe_strings`: every string added, and its span
    seen: Option<HashMap<String, [u32; 2]>>,
}

impl Heap {
    fn add(&mut self, s: &str) -> Result<[u32; 2], CompileError> {
        if let Some(&span) = self.seen.as_ref().and_then(|m| m.get(s)) {
            return Ok(span);
        }
        let a = u32::try_from(self.bytes.len()).map_err(|_| invalid("string heap over 4 GB"))?;
        self.bytes.extend_from_slice(s.as_bytes());
        let b = u32::try_from(self.bytes.len()).map_err(|_| invalid("string heap over 4 GB"))?;
        if let Some(m) = self.seen.as_mut() {
            m.insert(s.to_string(), [a, b]);
        }
        Ok([a, b])
    }
}

fn finite(what: &str, xs: &[f32]) -> Result<(), CompileError> {
    match xs.iter().find(|x| !x.is_finite()) {
        Some(x) => Err(invalid(format!("{what}: {x} is not a finite number"))),
        None => Ok(()),
    }
}

fn u32_len(n: usize, what: &str) -> Result<u32, CompileError> {
    u32::try_from(n).map_err(|_| invalid(format!("too many {what}")))
}

fn pair_code(p: Option<bool>) -> u32 {
    match p {
        Some(false) => 0,
        Some(true) => 1,
        None => UNSET,
    }
}

fn check_word(w: &str, what: &str) -> Result<(), CompileError> {
    if w.contains(['\t', '\n', '\r']) {
        return Err(invalid(format!("{what} word {w:?} has a tab or line break")));
    }
    Ok(())
}

/// Compile sources into the bytes of a `.kmj` file.
pub fn compile(src: &Sources) -> Result<Vec<u8>, CompileError> {
    if src.emotions.len() != N_EMO {
        return Err(invalid(format!("{} emotions given; the format has {N_EMO}", src.emotions.len())));
    }
    if src.grammar.is_empty() {
        return Err(invalid("no grammar word lists (data/grammar/<lang>.json)"));
    }
    let n = src.faces.len();
    let nt = src.vocab.len();
    let k = src.dense_k;
    if n >= (u32::MAX >> 2) as usize {
        return Err(invalid("too many faces"));
    }
    if src.dense.len() != nt * k || src.engine.as_ref().is_some_and(|e| e.len() != nt * k) {
        return Err(invalid("neighbour lists do not match the vocab (dense_k per term)"));
    }
    let enc = src.params.encoding;
    let mut heap = Heap { bytes: Vec::new(), seen: enc.dedupe_strings.then(HashMap::new) };

    // ---- faces, in id order --------------------------------------------
    let ids: Vec<u64> = src.faces.iter().map(|f| FaceId::of_text(&f.text).as_u64()).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (ids[i], i));
    if let Some(w) = order.windows(2).find(|w| ids[w[0]] == ids[w[1]]) {
        return Err(invalid(format!(
            "two faces share id k{:012x}: {:?} and {:?} (the same text twice, or a 48-bit collision)",
            ids[w[0]],
            src.faces[w[0]].text,
            src.faces[w[1]].text
        )));
    }
    // export index -> ordinal
    let mut ord = vec![0u32; n];
    for (o, &i) in order.iter().enumerate() {
        ord[i] = o as u32;
    }
    let mut fnum = Vec::with_capacity(n);
    let mut fids = Vec::with_capacity(n);
    let mut ftxt = Vec::with_capacity(n);
    let mut fchr = Vec::with_capacity(n);
    let mut chrs: Vec<u32> = Vec::new();
    let mut words: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for (o, &i) in order.iter().enumerate() {
        let f = &src.faces[i];
        finite(&format!("face {:?}", f.text), &f.r)?;
        finite(
            &format!("face {:?}", f.text),
            &[f.multi, f.cute, f.intensity, f.suggestive, f.lenny, f.face, f.quality, f.crude],
        )?;
        fnum.push(FaceNum {
            r: f.r,
            multi: f.multi,
            cute: f.cute,
            intensity: f.intensity,
            suggestive: f.suggestive,
            lenny: f.lenny,
            face: f.face,
            quality: f.quality,
            crude: f.crude,
            len: u32_len(f.text.chars().count(), "chars")?,
            flags: 0,
        });
        fids.push(ids[i]);
        ftxt.push(heap.add(&f.text)?);
        let a = u32_len(chrs.len(), "chars")?;
        chrs.extend(emoticond::text::char_set(&f.text).into_iter().map(|c| c as u32));
        fchr.push([a, u32_len(chrs.len(), "chars")?]);
        for (w, tier) in &f.words {
            words.entry(w.as_str()).or_default().push((o as u32) << 2 | u32::from((*tier).min(2)));
        }
    }
    let mut btxt: Vec<u32> = (0..n as u32).collect();
    btxt.sort_by(|&a, &b| src.faces[order[a as usize]].text.as_bytes().cmp(src.faces[order[b as usize]].text.as_bytes()));
    let text_index: HashMap<&str, usize> = src.faces.iter().enumerate().map(|(i, f)| (f.text.as_str(), i)).collect();

    // ---- vocab ---------------------------------------------------------
    let mut term = Vec::with_capacity(nt);
    let mut tkey = Vec::with_capacity(nt);
    let mut by_key: BTreeMap<&str, u32> = BTreeMap::new();
    for (t, v) in src.vocab.iter().enumerate() {
        finite(&format!("term {:?}", v.key), &v.p)?;
        finite(&format!("term {:?}", v.key), &[v.m.unwrap_or(0.0)])?;
        let mut flags = 0;
        if v.weak {
            flags |= TERM_WEAK;
        }
        if v.m.is_some() {
            flags |= TERM_HAS_M;
        }
        term.push(TermRec { p: v.p, m: v.m.unwrap_or(0.0), n: v.n, tier: v.tier, flags });
        tkey.push(heap.add(&v.key)?);
        by_key.insert(&v.key, t as u32);
    }
    let tsrt: Vec<u32> = by_key.into_values().collect();
    let remap = |l: &[u32]| -> Vec<u32> { l.iter().map(|&i| if (i as usize) < n { ord[i as usize] } else { NONE }).collect() };
    let dnse = remap(&src.dense);
    let engn = src.engine.as_deref().map(remap);

    // ---- tag postings (each list in ordinal order) -----------------------
    let mut wkey = Vec::with_capacity(words.len());
    let mut wpst = Vec::with_capacity(words.len());
    let mut post: Vec<u32> = Vec::new();
    let mut pstv: Vec<u8> = Vec::new();
    for (w, p) in &words {
        wkey.push(heap.add(w)?);
        match enc.postings {
            PostingsEncoding::U32 => {
                let a = u32_len(post.len(), "postings")?;
                post.extend_from_slice(p);
                wpst.push([a, u32_len(post.len(), "postings")?]);
            }
            PostingsEncoding::Varint => {
                // (gap from the previous ordinal << 2) | tier; ordinals ascend
                let a = u32_len(pstv.len(), "posting bytes")?;
                let mut prev = 0u64;
                for &x in p {
                    let o = u64::from(x >> 2);
                    put_varint(&mut pstv, ((o - prev) << 2) | u64::from(x & 3));
                    prev = o;
                }
                wpst.push([a, u32_len(pstv.len(), "posting bytes")?]);
            }
        }
    }

    // ---- string lists, phrases ------------------------------------------
    let mut slst: Vec<[u32; 2]> = Vec::new();
    let mut add_list = |heap: &mut Heap, l: &[&str]| -> Result<[u32; 2], CompileError> {
        let a = u32_len(slst.len(), "list strings")?;
        for s in l {
            slst.push(heap.add(s)?);
        }
        Ok([a, u32_len(slst.len(), "list strings")?])
    };
    let mut phrs = Vec::with_capacity(src.phrases.len());
    let mut spellings: BTreeMap<String, u32> = BTreeMap::new();
    let mut phrase_max = 1;
    for (i, ph) in src.phrases.iter().enumerate() {
        finite(&format!("phrase {:?}", ph.key), &ph.p)?;
        for m in ph.spellings.iter().chain(std::iter::once(&ph.key)) {
            let t = emoticond::tokens(m);
            if t.is_empty() {
                continue;
            }
            phrase_max = phrase_max.max(t.len());
            spellings.entry(t.join(" ")).or_insert(i as u32);
        }
        let mut flags = 0;
        for (on, f) in [(ph.cute, PHRASE_CUTE), (ph.lenny, PHRASE_LENNY), (ph.lewd, PHRASE_LEWD), (ph.over, PHRASE_OVER)] {
            if on {
                flags |= f;
            }
        }
        let words: Vec<&str> = ph.words.iter().map(String::as_str).collect();
        phrs.push(PhraseRec {
            p: ph.p,
            key: heap.add(&ph.key)?,
            words: add_list(&mut heap, &words)?,
            level: ph.level.map_or(UNSET, u32::from),
            pair: pair_code(ph.pair),
            flags,
        });
    }
    let mut pidx: Vec<[u32; 3]> = Vec::with_capacity(spellings.len());
    for (s, i) in &spellings {
        let [a, b] = heap.add(s)?;
        pidx.push([a, b, *i]);
    }

    // ---- situations, in name order ---------------------------------------
    let mut sits: Vec<&SituationIn> = src.situations.iter().collect();
    sits.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(w) = sits.windows(2).find(|w| w[0].name == w[1].name) {
        return Err(invalid(format!("situation {:?} is defined twice", w[0].name)));
    }
    let mut situ = Vec::with_capacity(sits.len());
    let mut smat: Vec<[u32; 2]> = Vec::new();
    for s in sits {
        finite(&format!("situation {:?}", s.name), &s.p)?;
        let a = u32_len(smat.len(), "situation spellings")?;
        for m in &s.matches {
            let t = emoticond::tokens(m);
            if !t.is_empty() {
                let t: Vec<&str> = t.iter().map(String::as_str).collect();
                smat.push(add_list(&mut heap, &t)?);
            }
        }
        let matches = [a, u32_len(smat.len(), "situation spellings")?];
        let words: Vec<&str> = s.words.iter().map(String::as_str).collect();
        situ.push(SituRec { p: s.p, name: heap.add(&s.name)?, matches, words: add_list(&mut heap, &words)?, pair: pair_code(s.pair) });
    }

    // ---- canonical picks: hand before agent, by rank, top 3 ---------------
    // Ties go to the export order (the pre-v3 engine's), then ids remap.
    let mut picks: BTreeMap<String, Vec<(bool, u64, usize)>> = BTreeMap::new();
    for c in &src.canonical {
        if let Some(&i) = text_index.get(c.text.as_str()) {
            picks.entry(c.term.to_lowercase()).or_default().push((!c.hand, c.rank, i));
        }
    }
    let mut cano = Vec::with_capacity(picks.len());
    for (t, mut v) in picks {
        v.sort();
        let mut seen = HashSet::new();
        let mut faces = [NONE; 3];
        let mut m = 0;
        for (_, _, i) in v {
            if m < 3 && seen.insert(i) {
                faces[m] = ord[i];
                m += 1;
            }
        }
        cano.push(CanonRec { term: heap.add(&t)?, faces, n: m as u32 });
    }

    // ---- boosts: the last (term, id) wins ---------------------------------
    let mut boost_map: BTreeMap<(&str, u64), f32> = BTreeMap::new();
    for b in &src.boosts {
        finite(&format!("boost {:?}", b.term), &[b.boost])?;
        boost_map.insert((&b.term, b.id.as_u64()), b.boost);
    }
    let n_boost_terms = boost_map.keys().map(|k| k.0).collect::<HashSet<_>>().len();
    let mut bost = Vec::with_capacity(boost_map.len());
    for ((t, id), b) in &boost_map {
        bost.push(BoostRec { term: heap.add(t)?, id: [*id as u32, (*id >> 32) as u32], boost: *b, reserved: 0 });
    }

    // ---- grammar, manifest ------------------------------------------------
    let mut grams: Vec<&Grammar> = src.grammar.iter().collect();
    grams.sort_by(|a, b| a.lang.cmp(&b.lang));
    for g in &grams {
        if g.lang.is_empty() || g.lang.contains(['=', '\n', ',']) {
            return Err(invalid(format!("bad grammar language {:?}", g.lang)));
        }
        for name in emoticond::grammar::LISTS {
            for w in g.list(name) {
                check_word(w, &format!("grammar {} {name}", g.lang))?;
            }
        }
    }
    let p = &src.params;
    finite("thresholds", &[p.innuendo_at, p.sexual_at, p.crude_at, p.quality_cut.unwrap_or(0.0)])?;
    let one_line = |k: &str, v: &str| -> Result<String, CompileError> {
        if v.contains(['\n', '\r']) {
            return Err(invalid(format!("manifest value for {k} has a line break")));
        }
        Ok(format!("{k}={v}\n"))
    };
    let aliases = alias_tables(&src.aliases, &fids)?;
    let minor = if enc.is_3_0() && aliases.0.is_empty() && aliases.1.is_empty() { 0 } else { FORMAT_MINOR };
    let mut mani = String::new();
    for (key, v) in [
        ("format", format!("{FORMAT_MAJOR}.{minor}")),
        ("data_version", p.data_version.clone()),
        ("set", p.dataset.clone()),
        ("build_date", p.build_date.clone()),
        ("source_rev", src.source_rev.clone()),
        ("policy", p.policy.clone()),
        ("licence", p.licence.clone()),
        ("n_faces", n.to_string()),
        ("n_terms", nt.to_string()),
        ("n_phrases", phrs.len().to_string()),
        ("n_situations", situ.len().to_string()),
        ("n_canonical", cano.len().to_string()),
        ("n_boost_terms", n_boost_terms.to_string()),
        ("dense_k", k.to_string()),
        ("emotions", src.emotions.join(",")),
        ("extras", src.extras.join(",")),
        ("quality_cut", p.quality_cut.map_or("none".to_string(), |q| q.to_string())),
        ("safety.innuendo_at", p.innuendo_at.to_string()),
        ("safety.sexual_at", p.sexual_at.to_string()),
        ("crude.at", p.crude_at.to_string()),
        ("long_at_default", p.long_at_default.to_string()),
        ("languages", grams.iter().map(|g| g.lang.as_str()).collect::<Vec<_>>().join(",")),
        ("phrase_max", phrase_max.min(8).to_string()),
        ("min_reader", emoticond::VERSION.to_string()),
    ] {
        mani.push_str(&one_line(key, &v)?);
    }
    if minor > 0 {
        // 3.1 keys (a 3.0 file's manifest stays as the first compiler wrote it)
        mani.push_str(&one_line("encoding", &enc.describe())?);
        mani.push_str(&one_line("n_aliases", &aliases.0.len().to_string())?);
        mani.push_str(&one_line("n_retired", &aliases.1.len().to_string())?);
    }
    if src.emotions.iter().chain(&src.extras).any(|e| e.contains([',', '=', '\n']) || e.is_empty()) {
        return Err(invalid("emotion and extra names must be non-empty, without commas"));
    }

    // ---- encode and assemble -------------------------------------------
    let (fnq, fncb) = match enc.nums {
        NumsEncoding::F32 => (Vec::new(), Vec::new()),
        NumsEncoding::Q16 => code_nums(&fnum, 2, &[])?,
        NumsEncoding::Q8 => {
            // thresholds the engine tests, per coded field (after the 19 emotions)
            let mut cuts: Vec<(usize, Vec<f32>)> = Vec::new();
            let x = N_EMO;
            cuts.push((x, vec![0.5])); // multi > 0.5
            cuts.push((x + 3, vec![p.innuendo_at, p.sexual_at])); // suggestive
            cuts.push((x + 4, vec![0.5])); // lenny >= 0.5
            cuts.push((x + 5, vec![0.5])); // face < 0.5
            cuts.push((x + 7, vec![p.crude_at])); // crude
            code_nums(&fnum, 1, &cuts)?
        }
    };
    let packed = |l: &[u32]| pack_lists(l, n, k, nt);
    let (dnsp, engp) = match enc.lists {
        ListEncoding::U32 => (Vec::new(), None),
        ListEncoding::Packed => (packed(&dnse), engn.as_deref().map(packed)),
    };
    let gram_text: Vec<String> = grams.iter().map(|g| g.to_section()).collect();
    let mut sections: Vec<(Tag, &[u8])> = vec![
        (*b"MANI", mani.as_bytes()),
        (*b"LICN", src.licence.as_bytes()),
        (*b"ATTR", src.attribution.as_bytes()),
    ];
    for g in &gram_text {
        sections.push((*b"GRAM", g.as_bytes()));
    }
    match enc.nums {
        NumsEncoding::F32 => sections.push((*b"FNUM", as_bytes(&fnum))),
        NumsEncoding::Q16 => sections.extend([(*b"FNQ2", &fnq[..]), (*b"FNCB", as_bytes(&fncb))]),
        NumsEncoding::Q8 => sections.extend([(*b"FNQ1", &fnq[..]), (*b"FNCB", as_bytes(&fncb))]),
    }
    sections.extend([(*b"FIDS", as_bytes(&fids)), (*b"FTXT", as_bytes(&ftxt))]);
    if enc.store_chars {
        sections.extend([(*b"FCHR", as_bytes(&fchr)), (*b"CHRS", as_bytes(&chrs))]);
    }
    sections.extend([
        (*b"BTXT", as_bytes(&btxt)),
        (*b"TERM", as_bytes(&term)),
        (*b"TKEY", as_bytes(&tkey)),
        (*b"TSRT", as_bytes(&tsrt)),
    ]);
    match enc.lists {
        ListEncoding::U32 => {
            sections.push((*b"DNSE", as_bytes(&dnse)));
            if let Some(e) = &engn {
                sections.push((*b"ENGN", as_bytes(e)));
            }
        }
        ListEncoding::Packed => {
            sections.push((*b"DNSP", as_bytes(&dnsp)));
            if let Some(e) = &engp {
                sections.push((*b"ENGP", as_bytes(e)));
            }
        }
    }
    sections.extend([(*b"WKEY", as_bytes(&wkey)), (*b"WPST", as_bytes(&wpst))]);
    match enc.postings {
        PostingsEncoding::U32 => sections.push((*b"POST", as_bytes(&post))),
        PostingsEncoding::Varint => sections.push((*b"PSTV", &pstv[..])),
    }
    sections.extend([
        (*b"SLST", as_bytes(&slst)),
        (*b"PHRS", as_bytes(&phrs)),
        (*b"PIDX", as_bytes(&pidx)),
        (*b"SITU", as_bytes(&situ)),
        (*b"SMAT", as_bytes(&smat)),
        (*b"CANO", as_bytes(&cano)),
    ]);
    if !bost.is_empty() {
        sections.push((*b"BOST", as_bytes(&bost)));
    }
    if !aliases.0.is_empty() {
        sections.push((*b"ALIA", as_bytes(&aliases.0)));
    }
    if !aliases.1.is_empty() {
        sections.push((*b"RETD", as_bytes(&aliases.1)));
    }
    sections.push((*b"STRS", &heap.bytes));
    let mut flags = 0;
    if p.policy.starts_with("public") {
        flags |= FLAG_PUBLIC;
    }
    if !aliases.0.is_empty() {
        flags |= FLAG_ALIASES;
    }
    Ok(assemble(&sections, flags, minor))
}

/// `FNQ2`/`FNQ1` records and their `FNCB` tables. `width` is the code size
/// in bytes (2: every distinct value gets a code, lossless; 1: at most 256
/// codes per field). `cuts` are, per coded field, values no quantisation
/// bin may straddle (a bin holds only values on one side of each, and a
/// value equal to it alone).
fn code_nums(fnum: &[FaceNum], width: usize, cuts: &[(usize, Vec<f32>)]) -> Result<(Vec<u8>, Vec<u32>), CompileError> {
    let cap = if width == 2 { 1usize << 16 } else { 256 };
    let n = fnum.len();
    let coded: Vec<[f32; N_CODED]> = fnum.iter().map(FaceNum::coded).collect();
    let mut codes = vec![[0u16; N_CODED]; n];
    let mut tables: Vec<Vec<f32>> = Vec::with_capacity(N_CODED);
    for f in 0..N_CODED {
        // distinct values (by bits, so the round trip is exact) with counts
        let mut vals: Vec<(f32, usize)> = coded.iter().map(|c| (c[f], 0)).collect();
        vals.sort_by(|a, b| a.0.total_cmp(&b.0));
        vals.dedup_by(|a, b| a.0.to_bits() == b.0.to_bits());
        let index = |x: f32, vals: &[(f32, usize)]| vals.binary_search_by(|v| v.0.total_cmp(&x)).unwrap_or(0);
        for c in &coded {
            let i = index(c[f], &vals);
            vals[i].1 += 1;
        }
        // value index -> code, and the table
        let (code_of, table): (Vec<u16>, Vec<f32>) = if vals.len() <= cap {
            ((0..vals.len()).map(|i| i as u16).collect(), vals.iter().map(|v| v.0).collect())
        } else if width == 2 {
            return Err(invalid(format!("face field {f} has {} distinct values; q16 holds 65,536", vals.len())));
        } else {
            let cut: &[f32] = cuts.iter().find(|c| c.0 == f).map_or(&[], |c| &c.1);
            let bins = (256 - 2 * cut.len() - 1) as f64;
            let (lo, hi) = (f64::from(vals[0].0), f64::from(vals[vals.len() - 1].0));
            let key = |x: f32| {
                let side: usize = cut.iter().map(|&t| usize::from(x > t) + usize::from(x >= t)).sum();
                let b = if hi > lo { ((f64::from(x) - lo) / (hi - lo) * bins).floor().min(bins - 1.0) as usize } else { 0 };
                (side, b)
            };
            let mut code_of = Vec::with_capacity(vals.len());
            let mut table = Vec::new();
            let mut groups: Vec<(f64, usize, f32, f32)> = Vec::new(); // (sum, count, min, max)
            let mut last = None;
            for &(x, cnt) in &vals {
                let k = key(x);
                if last != Some(k) {
                    groups.push((0.0, 0, x, x));
                    last = Some(k);
                }
                let g = groups.last_mut().expect("pushed above");
                g.0 += f64::from(x) * cnt as f64;
                g.1 += cnt;
                g.3 = x;
                code_of.push((groups.len() - 1) as u16);
            }
            for (sum, cnt, a, b) in groups {
                let mean = if cnt > 0 { (sum / cnt as f64) as f32 } else { a };
                table.push(mean.clamp(a, b));
            }
            debug_assert!(table.len() <= 256);
            (code_of, table)
        };
        for (c, out) in coded.iter().zip(codes.iter_mut()) {
            out[f] = code_of[index(c[f], &vals)];
        }
        tables.push(table);
    }
    let stride = if width == 2 { FACE_Q2_LEN } else { FACE_Q1_LEN };
    let mut rec = Vec::with_capacity(n * stride);
    for (c, f) in codes.iter().zip(fnum) {
        for &x in c {
            if width == 2 {
                rec.extend_from_slice(&x.to_le_bytes());
            } else {
                rec.push(x as u8);
            }
        }
        if width == 1 {
            rec.push(0);
        }
        rec.extend_from_slice(&(f.len.min(u32::from(u16::MAX)) as u16).to_le_bytes());
    }
    let mut book: Vec<u32> = Vec::with_capacity(N_CODED + 1);
    let mut at = 0u32;
    book.push(0);
    for t in &tables {
        at += t.len() as u32;
        book.push(at);
    }
    book.extend(tables.iter().flatten().map(|x| x.to_bits()));
    Ok((rec, book))
}

/// `DNSP`/`ENGP` words for neighbour lists of `k` ordinals per term.
fn pack_lists(lists: &[u32], n: usize, k: usize, nt: usize) -> Vec<u32> {
    let bits = packed_bits(n);
    let mut starts = Vec::with_capacity(nt + 1);
    let mut slots = Vec::with_capacity(lists.len());
    starts.push(0u32);
    for t in 0..nt {
        let l = &lists[t * k..(t + 1) * k];
        let used = l.iter().rposition(|&x| x != NONE).map_or(0, |p| p + 1);
        slots.extend_from_slice(&l[..used]);
        starts.push(slots.len() as u32);
    }
    let mut w = vec![bits, nt as u32];
    w.extend(starts);
    w.extend(pack_slots(&slots, bits));
    w
}

/// `ALIA` (old, new) sorted by old, and `RETD` (id, reason) sorted by id.
/// Chains are followed to their end; a cycle, an old id that is a face of
/// this set, or an id both renamed and retired is an error.
fn alias_tables(aliases: &[AliasIn], fids: &[u64]) -> Result<AliasTables, CompileError> {
    let live = |id: u64| fids.binary_search(&id).is_ok();
    let mut to: BTreeMap<u64, u64> = BTreeMap::new();
    let mut retired: BTreeMap<u64, u32> = BTreeMap::new();
    for a in aliases {
        let old = a.old.as_u64();
        if live(old) {
            return Err(invalid(format!("alias: k{old:012x} is a face of this set; it cannot be renamed or retired")));
        }
        match a.new {
            Some(new) if new.as_u64() == old => return Err(invalid(format!("alias: k{old:012x} renamed to itself"))),
            Some(new) => {
                if to.insert(old, new.as_u64()).is_some_and(|x| x != new.as_u64()) {
                    return Err(invalid(format!("alias: k{old:012x} renamed twice")));
                }
            }
            None => {
                retired.insert(old, a.reason);
            }
        }
    }
    if let Some(id) = to.keys().find(|id| retired.contains_key(id)) {
        return Err(invalid(format!("alias: k{id:012x} is both renamed and retired")));
    }
    let mut alia = Vec::with_capacity(to.len());
    for (&old, &first) in &to {
        let mut new = first;
        let mut hops = 0;
        while let Some(&next) = to.get(&new) {
            new = next;
            hops += 1;
            if hops > to.len() {
                return Err(invalid(format!("alias: k{old:012x} is part of a cycle")));
            }
        }
        alia.push([old, new]);
    }
    let retd = retired.into_iter().map(|(id, r)| [id, u64::from(r)]).collect();
    Ok((alia, retd))
}

/// Lay out header, directory and sections (8-aligned, zero padding).
fn assemble(sections: &[(Tag, &[u8])], flags: u32, minor: u16) -> Vec<u8> {
    let header_len = HEADER_LEN + sections.len() * DIR_ENTRY_LEN;
    let mut off = header_len.next_multiple_of(8);
    let mut dir = Vec::with_capacity(sections.len());
    for (tag, data) in sections {
        let (elem, required) = SECTIONS.iter().find(|s| s.0 == *tag).map_or((1, false), |s| (s.2, s.3));
        dir.push(SectionEntry {
            tag: *tag,
            flags: if required { SECTION_REQUIRED } else { 0 },
            offset: off as u64,
            len: data.len() as u64,
            elem_size: elem as u32,
            crc32: crc32(data),
        });
        off = (off + data.len()).next_multiple_of(8);
    }
    let mut out = Vec::with_capacity(off);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_MAJOR.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(header_len as u64).to_le_bytes());
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 32]); // content hash, below
    for e in &dir {
        out.extend_from_slice(&e.to_bytes());
    }
    for (_, data) in sections {
        out.resize(out.len().next_multiple_of(8), 0);
        out.extend_from_slice(data);
    }
    out.resize(out.len().next_multiple_of(8), 0);
    let hash = Sha256::digest(&out[HEADER_LEN..]);
    out[32..64].copy_from_slice(&hash);
    out
}

/// Write bytes to `path` atomically: a temp file beside it, synced, then
/// renamed over it (a reader that has the old file mapped keeps it).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.kmj".into());
    let tmp = dir.join(format!(".{name}.tmp{}", std::process::id()));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// SHA-256 of some bytes, in hex.
pub fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 over a stream of length-prefixed parts.
#[derive(Default)]
pub(crate) struct Sha256Stream(Sha256);

impl Sha256Stream {
    pub(crate) fn update(&mut self, b: &[u8]) {
        self.0.update((b.len() as u64).to_le_bytes());
        self.0.update(b);
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.0.finalize().to_vec()
    }
}
