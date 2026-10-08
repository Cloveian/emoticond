//! [`Database`]: the library's entry point (docs/api-frontends.md §1.2).

use crate::data::format::FaceNum;
use crate::data::{DataBytes, DataFile, Manifest};
use crate::emo::{Ctx, EmoDb, N_EMO};
use crate::error::{OpenError, Warning};
use crate::id::FaceId;
use crate::options::{OpenOptions, OverlaySource, SearchOptions};
use crate::overlay::OverlayTables;
use crate::policy::Policy;
use crate::result::{Attrs, Completion, CompletionSource, DbInfo, Entry, Flags, Hit, Reading, SearchResult, TermSource};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// Query text longer than this (in chars) is cut before parsing.
const MAX_QUERY_CHARS: usize = 256;

struct Inner {
    emo: EmoDb,
    /// the file opened, if opened from a path
    path: Option<PathBuf>,
    policy: Policy,
    /// face ordinal -> concepts the shipped data pins it for (sorted)
    canonical_for: BTreeMap<u32, Vec<String>>,
    warnings: Vec<Warning>,
    /// What can change without reopening: swapped whole, so a search reads
    /// one consistent snapshot (it clones the `Arc` at its start).
    live: RwLock<Arc<Live>>,
    /// Serialises the setters, so two concurrent changes both land.
    writer: Mutex<()>,
}

/// The tables `set_blocklist`, `reload_overlays` and `set_defaults` swap.
struct Live {
    blocklist: BTreeSet<FaceId>,
    overlays: Arc<OverlayTables>,
    /// the blocklist and the overlays' hidden faces, as ordinals
    blocked: HashSet<u32>,
    defaults: SearchOptions,
}

impl Live {
    fn new(e: &EmoDb, blocklist: BTreeSet<FaceId>, overlays: Arc<OverlayTables>, defaults: SearchOptions) -> Live {
        let blocked = blocklist.iter().chain(&overlays.hidden).filter_map(|id| e.st.by_fid(*id)).collect();
        Live { blocklist, overlays, blocked, defaults }
    }
}

/// An open kaomoji database. Cheap to clone (an `Arc`), `Send + Sync`,
/// and every method takes `&self`: share one across threads.
///
/// Searching never fails; any string is a valid query. The same data, text
/// and options always give the same result, byte for byte.
///
/// The blocklist, overlays and default options can change while it is in
/// use ([`set_blocklist`](Database::set_blocklist),
/// [`reload_overlays`](Database::reload_overlays),
/// [`set_defaults`](Database::set_defaults)): each is an atomic swap that
/// every clone sees, and a search in flight finishes on the values it
/// started with.
#[derive(Clone)]
pub struct Database {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").field("path", &self.inner.path).field("faces", &self.inner.emo.n_faces()).finish()
    }
}

fn flags_of(e: &EmoDb, f: &FaceNum, long_at: u32) -> Flags {
    let mut fl = Flags::empty();
    fl.set(Flags::SUGGESTIVE, f.suggestive >= e.innuendo_at);
    fl.set(Flags::EXPLICIT, f.suggestive >= e.sexual_at);
    fl.set(Flags::LENNY, f.lenny >= 0.5);
    fl.set(Flags::CRUDE, f.crude >= e.crude_at);
    fl.set(Flags::MULTI, f.multi > 0.5);
    fl.set(Flags::NOT_FACE, f.face < 0.5);
    fl.set(Flags::LONG, f.len > long_at);
    fl
}

/// The data file names a dataset choice tries, best first.
#[cfg(feature = "fs")]
fn candidates(d: crate::options::Dataset) -> [&'static str; 2] {
    use crate::options::Dataset;
    match d {
        Dataset::Core => ["core.kmj", "full.kmj"],
        Dataset::Full | Dataset::Auto => ["full.kmj", "core.kmj"],
    }
}

/// How many canonical faces lead an empty query before the user's history.
pub const BROWSE_STARTER: usize = 20;

impl Database {
    /// Open a data set from disk: `opts.file` if given, else the first of
    /// `opts.data_dirs` holding one. In each dir the `dataset` choice is
    /// tried first (`core.kmj` for `Core`, `full.kmj` for `Full` and
    /// `Auto`), then the other set, with a warning.
    ///
    /// The file is memory-mapped (feature `mmap`) or read into memory.
    /// Nothing is parsed or compiled: opening checks the header and section
    /// directory and takes about a millisecond.
    #[cfg(feature = "fs")]
    pub fn open(opts: OpenOptions) -> Result<Database, OpenError> {
        if let Some(f) = opts.file.clone() {
            if !f.is_file() {
                return Err(OpenError::NotFound { searched: vec![f] });
            }
            let st = DataFile::open(&f)?;
            return Database::with_data(st, Some(f), opts, Vec::new());
        }
        let mut searched = Vec::new();
        for dir in &opts.data_dirs {
            for (k, name) in candidates(opts.dataset).into_iter().enumerate() {
                let p = dir.join(name);
                if !p.is_file() {
                    searched.push(p);
                    continue;
                }
                let st = DataFile::open(&p)?;
                let mut warnings = Vec::new();
                if k > 0 {
                    warnings.push(Warning::new(
                        "dataset_fallback",
                        Some("dataset"),
                        format!("{} is not installed in {}; using {name}", candidates(opts.dataset)[0], dir.display()),
                    ));
                }
                return Database::with_data(st, Some(p), opts, warnings);
            }
        }
        Err(OpenError::NotFound { searched })
    }

    /// Open a data set already in memory: an owned buffer, or a `&'static`
    /// slice (`include_bytes!`, see [`include_data!`](crate::include_data)).
    /// Needs no file system (works with default features off); `data_dirs`,
    /// `file` and `dataset` are ignored.
    pub fn from_bytes(bytes: impl Into<DataBytes>, opts: OpenOptions) -> Result<Database, OpenError> {
        let st = DataFile::from_bytes(bytes)?;
        Database::with_data(st, None, opts, Vec::new())
    }

    fn with_data(st: DataFile, path: Option<PathBuf>, opts: OpenOptions, mut warnings: Vec<Warning>) -> Result<Database, OpenError> {
        let dv = st.manifest().data_version.clone();
        if !crate::data_compatible(&dv) {
            return Err(OpenError::Incompatible {
                found: format!("data version {dv}"),
                supported: concat!("data versions ", env!("CARGO_PKG_VERSION_MAJOR"), ".x"),
            });
        }
        let emo = EmoDb::new(st);
        let overlays = if opts.overlays.is_empty() {
            Arc::new(OverlayTables::default())
        } else {
            let (t, mut w) = Self::build_overlays(&emo, &opts.overlays);
            warnings.append(&mut w);
            Arc::new(t)
        };
        let mut canonical_for: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for (term, list) in emo.st.canonical_all() {
            for (fi, _) in list {
                canonical_for.entry(fi).or_default().push(term.to_string());
            }
        }
        for v in canonical_for.values_mut() {
            v.sort();
        }
        let live = Live::new(&emo, opts.blocklist, overlays, opts.defaults);
        Ok(Database {
            inner: Arc::new(Inner {
                emo,
                path,
                policy: opts.policy,
                canonical_for,
                warnings,
                live: RwLock::new(Arc::new(live)),
                writer: Mutex::new(()),
            }),
        })
    }

    fn build_overlays(emo: &EmoDb, sources: &[OverlaySource]) -> (OverlayTables, Vec<Warning>) {
        let (layers, mut w) = crate::overlay::load_sources(sources);
        let (t, mut w2) = crate::overlay::resolve(emo, &layers);
        w.append(&mut w2);
        (t, w)
    }

    /// The current snapshot of the swappable tables.
    fn live(&self) -> Arc<Live> {
        self.inner.live.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Build a new snapshot from the current one and swap it in. Setters
    /// are serialised; searches only ever wait for the pointer swap.
    fn update(&self, f: impl FnOnce(&Live) -> Live) {
        let _w = self.inner.writer.lock().unwrap_or_else(|e| e.into_inner());
        let next = Arc::new(f(&self.live()));
        *self.inner.live.write().unwrap_or_else(|e| e.into_inner()) = next;
    }

    /// Problems found at open that did not stop it (bad overlay lines,
    /// overlay rows naming faces this data set lacks, a data set fallback).
    /// Later overlay reloads return their own warnings.
    pub fn warnings(&self) -> &[Warning] {
        &self.inner.warnings
    }

    /// The default search options (`OpenOptions::defaults`, or the last
    /// `set_defaults`): start each query from these.
    pub fn defaults(&self) -> SearchOptions {
        self.live().defaults.clone()
    }

    /// Replace the default search options (after the user's config
    /// changed). Atomic: searches in flight are unaffected.
    pub fn set_defaults(&self, defaults: SearchOptions) {
        self.update(|l| Live { blocklist: l.blocklist.clone(), overlays: l.overlays.clone(), blocked: l.blocked.clone(), defaults });
    }

    /// Replace the blocklist: faces never returned, whatever the options
    /// (`OpenOptions::blocklist`). The overlays' hidden faces stay hidden.
    /// Atomic: searches in flight finish on the old list. Ids not in this
    /// data set are kept (they may be in another) and do nothing.
    pub fn set_blocklist(&self, ids: BTreeSet<FaceId>) {
        let e = &self.inner.emo;
        self.update(|l| Live::new(e, ids, l.overlays.clone(), l.defaults.clone()));
    }

    /// The blocklist in force (`OpenOptions::blocklist` or the last
    /// `set_blocklist`).
    pub fn blocklist(&self) -> BTreeSet<FaceId> {
        self.live().blocklist.clone()
    }

    /// Re-read the overlays (all of them: the list replaces the one given
    /// at open) and swap them in. Costs O(overlay size); the data file is
    /// not touched. Returns the problems found (bad lines, unknown faces,
    /// unknown alias targets); nothing here fails. Atomic: searches in
    /// flight finish on the old overlays.
    ///
    /// Front-ends that share state through files call this when an overlay
    /// file changed (`emoticond_state::Shared::refresh` reports `overlays`),
    /// watching [`OverlaySource::watch_paths`].
    pub fn reload_overlays(&self, overlays: &[OverlaySource]) -> Vec<Warning> {
        let e = &self.inner.emo;
        let (t, w) = Self::build_overlays(e, overlays);
        let t = Arc::new(t);
        self.update(|l| Live::new(e, l.blocklist.clone(), t, l.defaults.clone()));
        w
    }

    /// How many rows the overlays in force hold, for a status line:
    /// (pinned terms, phrases, boosted terms, hidden faces).
    pub fn overlay_counts(&self) -> (usize, usize, usize, usize) {
        let l = self.live();
        let o = &l.overlays;
        (o.canon.values().filter(|c| c.has_user()).count(), o.phrases.len(), o.boosts.len(), o.hidden.len())
    }

    /// The policy every query is clamped to.
    pub fn policy(&self) -> &Policy {
        &self.inner.policy
    }

    /// What is open: versions, data set, counts, emotion names.
    pub fn info(&self) -> DbInfo {
        let e = &self.inner.emo;
        let h = e.st.header();
        let m = e.st.manifest();
        let path = self.inner.path.as_ref();
        DbInfo {
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            format: format!("{}.{}", h.format_major, h.format_minor),
            data_dir: path.and_then(|p| p.parent()).map(|d| d.display().to_string()).unwrap_or_default(),
            path: path.map(|p| p.display().to_string()).unwrap_or_default(),
            data_version: m.data_version.clone(),
            dataset: m.set.clone(),
            licence: m.licence.clone(),
            content_hash: h.content_hash.iter().map(|b| format!("{b:02x}")).collect(),
            bytes: e.st.len() as u64,
            faces: e.n_faces(),
            terms: e.n_terms(),
            situations: e.n_situations(),
            phrases: e.n_phrases(),
            canonical: e.n_canonical(),
            boosts: e.st.n_boost_terms(),
            emotions: e.emotions.clone(),
        }
    }

    /// The data set's manifest (`MANI`): provenance, counts, thresholds.
    pub fn manifest(&self) -> &Manifest {
        self.inner.emo.st.manifest()
    }

    /// The data licence text (`LICN`), to show with the data.
    pub fn licence(&self) -> &str {
        self.inner.emo.st.licence()
    }

    /// Attribution and notices for the data's sources (`ATTR`).
    pub fn attribution(&self) -> &str {
        self.inner.emo.st.attribution()
    }

    /// Check every section's checksum (reads the whole file; open does not).
    /// On damage, returns the tag of the first bad section.
    pub fn verify(&self) -> Result<(), String> {
        self.inner.emo.st.verify()
    }

    /// Validate, clamp to policy, and resolve the options for one query.
    fn prepare(&self, opts: &SearchOptions) -> (SearchOptions, Vec<Warning>, Vec<&'static str>) {
        let mut eff = opts.clone();
        let warnings = eff.validate();
        let clamped = self.inner.policy.clamp(&mut eff);
        (eff, warnings, clamped)
    }

    fn ctx<'a>(&'a self, live: &'a Live, eff: &'a SearchOptions, warnings: &mut Vec<Warning>) -> Ctx<'a> {
        let mut unknown = Vec::new();
        let ctx = Ctx::new(&self.inner.emo, eff, &live.blocked, &live.overlays, &mut unknown);
        for (key, name) in unknown {
            warnings.push(Warning::new("unknown_emotion", Some(key), format!("unknown emotion {name:?} ignored")));
        }
        if ctx.impossible {
            warnings.push(Warning::new("contradiction", Some("emotions"), "emotions.min is above emotions.max: nothing can match"));
        }
        ctx
    }

    fn cut(text: &str) -> &str {
        match text.char_indices().nth(MAX_QUERY_CHARS) {
            Some((i, _)) => &text[..i],
            None => text,
        }
    }

    /// Search. An empty query browses (canonical picks, then the caller's
    /// usage), unless `emotions.target` is set, which ranks by it.
    pub fn search(&self, text: &str, opts: &SearchOptions) -> SearchResult {
        let text = Self::cut(text);
        let live = self.live();
        let (eff, mut warnings, clamped) = self.prepare(opts);
        let ctx = self.ctx(&live, &eff, &mut warnings);
        if text.trim().is_empty() && ctx.target.is_none() {
            drop(ctx);
            let mut r = self.browse_inner(&live, &eff);
            warnings.append(&mut r.warnings);
            r.warnings = warnings;
            r.clamped = clamped.into_iter().map(Cow::Borrowed).collect();
            return r;
        }
        let e = &self.inner.emo;
        let found = e.search(text, &ctx);
        let nums = e.st.nums();
        let mut why = found.why.into_iter();
        let hits = found
            .hits
            .iter()
            .enumerate()
            .map(|(k, &(i, score, pinned, user_pin))| {
                let mut flags = flags_of(e, &nums.at(i), ctx.long_at);
                flags.set(Flags::PINNED, pinned);
                flags.set(Flags::USER_PIN, user_pin);
                Hit {
                    id: e.st.fid(i),
                    text: e.st.text(i).to_string(),
                    score,
                    rank: (ctx.offset + k) as u32,
                    flags,
                    why: why.next().flatten(),
                }
            })
            .collect();
        let reading = (eff.explain != crate::options::Explain::Off).then(|| e.reading(text, &ctx));
        SearchResult {
            hits,
            corrected: found.corrected,
            reading,
            term_source: match (&found.term_key, found.term_user) {
                (None, _) => TermSource::None,
                (Some(_), false) => TermSource::Shipped,
                (Some(_), true) => TermSource::Overlay,
            },
            term_key: found.term_key,
            clamped: clamped.into_iter().map(Cow::Borrowed).collect(),
            warnings,
            safety: eff.safety,
            styles: eff.styles,
        }
    }

    /// How a query is read (the reading line), without searching.
    /// `explain = full` also fills `Reading::debug`.
    pub fn read(&self, text: &str, opts: &SearchOptions) -> Reading {
        let text = Self::cut(text);
        let live = self.live();
        let (eff, mut warnings, _) = self.prepare(opts);
        let ctx = self.ctx(&live, &eff, &mut warnings);
        self.inner.emo.reading(text, &ctx)
    }

    /// The empty-query screen: the canonical picks (each concept's first
    /// face, concepts with the most tagged faces first), then the faces the
    /// caller uses most (`usage.global`). Filters, `limit` and `offset` apply.
    /// An empty query: the first [`BROWSE_STARTER`] canonical faces (user
    /// pins first), then the user's history (usage), then the other
    /// canonical faces.
    pub fn browse(&self, opts: &SearchOptions) -> SearchResult {
        let (eff, warnings, clamped) = self.prepare(opts);
        let mut r = self.browse_inner(&self.live(), &eff);
        let mut warnings = warnings;
        warnings.append(&mut r.warnings);
        r.warnings = warnings;
        r.clamped = clamped.into_iter().map(Cow::Borrowed).collect();
        r
    }

    fn browse_inner(&self, live: &Live, eff: &SearchOptions) -> SearchResult {
        let mut w = Vec::new();
        let ctx = self.ctx(live, eff, &mut w);
        let e = &self.inner.emo;
        let ov = &live.overlays;
        // (face, pinned, pinned by the user)
        let mut order: Vec<(u32, bool, bool)> = Vec::new();
        if eff.pinned {
            // terms the user pinned faces for first, then by how many faces
            // carry the term
            let n_of = |k: &str| e.st.term_index(k).map_or(0, |t| e.st.term(t).n);
            let mut terms: Vec<(bool, &str, u32, Option<u32>)> = e
                .st
                .canonical_all()
                .filter(|(k, _)| !ov.canon.contains_key(*k))
                .map(|(k, list)| (false, k, n_of(k), list.first().map(|f| f.0)))
                .collect();
            for (k, l) in &ov.canon {
                let first = l.faces.first();
                terms.push((first.is_some_and(|f| f.1), k.as_str(), n_of(k), first.map(|f| f.0)));
            }
            terms.sort_by(|a, b| b.0.cmp(&a.0).then(b.2.cmp(&a.2)).then_with(|| a.1.cmp(b.1)));
            for (user, _, _, first) in terms {
                if let Some(fi) = first {
                    order.push((fi, true, user));
                }
            }
        }
        if eff.usage_weight != crate::options::UsageWeight::Off {
            let mut used: Vec<(f32, FaceId)> = eff.usage.global.iter().map(|(id, w)| (*w, *id)).collect();
            used.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            // a starter set of canonical faces, then the user's history, then
            // the rest of the canonical faces: with ~150 concepts pinned, the
            // history would otherwise never reach a page
            let rest = order.split_off(order.len().min(BROWSE_STARTER));
            order.extend(used.into_iter().filter_map(|(_, id)| Some((e.st.by_fid(id)?, false, false))));
            order.extend(rest);
        }
        let nums = e.st.nums();
        let mut seen = HashSet::new();
        let hits: Vec<Hit> = order
            .into_iter()
            .filter(|&(fi, _, _)| seen.insert(fi) && e.admissible_pub(fi as usize, &ctx))
            .skip(ctx.offset)
            .take(ctx.limit)
            .enumerate()
            .map(|(k, (fi, pinned, user_pin))| {
                let i = fi as usize;
                let mut flags = flags_of(e, &nums.at(i), ctx.long_at);
                flags.set(Flags::PINNED, pinned);
                flags.set(Flags::USER_PIN, user_pin);
                let rank = ctx.offset + k;
                Hit { id: e.st.fid(i), text: e.st.text(i).to_string(), score: 1.0 - rank as f32 * 0.001, rank: rank as u32, flags, why: None }
            })
            .collect();
        let reading = (eff.explain != crate::options::Explain::Off).then(Reading::default);
        SearchResult { hits, reading, warnings: w, safety: eff.safety, styles: eff.styles, ..SearchResult::default() }
    }

    /// Typeahead over vocabulary terms: concepts with pinned faces first,
    /// then emotion words, lexicon keys, and tags by how many faces carry
    /// them.
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<Completion> {
        let p = prefix.trim().to_lowercase();
        if p.is_empty() {
            return Vec::new();
        }
        let e = &self.inner.emo;
        let live = self.live();
        let ov = &live.overlays;
        e.completions(ov, &p, limit)
            .into_iter()
            .map(|c| Completion {
                text: c.to_string(),
                source: if ov.spell.contains_key(c) { CompletionSource::Overlay } else { CompletionSource::Term },
                pinned: e.has_canonical(ov, c),
            })
            .collect()
    }

    fn entry(&self, i: usize) -> Entry {
        let e = &self.inner.emo;
        let f = e.st.nums().at(i);
        Entry {
            id: e.st.fid(i),
            text: e.st.text(i).to_string(),
            quality: f.quality,
            flags: flags_of(e, &f, u32::from(e.st.manifest().long_at_default)),
            attrs: Attrs {
                multi: f.multi,
                cute: f.cute,
                intensity: f.intensity,
                suggestive: f.suggestive,
                lenny: f.lenny,
                face: f.face,
            },
            emotions: (0..N_EMO).map(|k| (e.emotions[k].clone(), f.r[k])).collect(),
            canonical_for: self.canonical_for(i as u32),
        }
    }

    /// The concepts face `i` is pinned for, after the overlays (sorted).
    fn canonical_for(&self, i: u32) -> Vec<String> {
        let mut v = self.inner.canonical_for.get(&i).cloned().unwrap_or_default();
        let live = self.live();
        let ov = &live.overlays.canon;
        if !ov.is_empty() {
            v.retain(|t| !ov.contains_key(t));
            v.extend(ov.iter().filter(|(_, l)| l.faces.iter().any(|f| f.0 == i)).map(|(t, _)| t.clone()));
            v.sort();
        }
        v
    }

    /// The face with this id.
    pub fn get(&self, id: FaceId) -> Option<Entry> {
        self.inner.emo.st.by_fid(id).map(|i| self.entry(i as usize))
    }

    /// What the data knows about an id: a face of this set, an old id
    /// renamed in a later release (`ALIA`; [`get`](Database::get), `exclude`,
    /// usage, blocklists and overlays follow it), a retired id (`RETD`), or
    /// unknown.
    pub fn id_status(&self, id: FaceId) -> crate::data::IdStatus {
        self.inner.emo.st.id_status(id)
    }

    /// The face with exactly this text (trimmed of surrounding whitespace
    /// if the exact text is not found).
    pub fn find_text(&self, text: &str) -> Option<Entry> {
        let st = &self.inner.emo.st;
        st.find_text(text).or_else(|| st.find_text(text.trim())).map(|i| self.entry(i as usize))
    }

    /// Every face, in id order.
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        self.inner.emo.st.ordinals_by_fid().map(move |i| self.entry(i as usize))
    }

    /// Faces that feel like this one: nearest by emotion profile and
    /// attributes, thinned of near-duplicates. Options apply as in `search`.
    pub fn similar(&self, id: FaceId, opts: &SearchOptions) -> SearchResult {
        let live = self.live();
        let (eff, mut warnings, clamped) = self.prepare(opts);
        let ctx = self.ctx(&live, &eff, &mut warnings);
        let e = &self.inner.emo;
        let mut result = SearchResult {
            warnings,
            clamped: clamped.into_iter().map(Cow::Borrowed).collect(),
            safety: eff.safety,
            styles: eff.styles,
            ..SearchResult::default()
        };
        let Some(src) = e.st.by_fid(id) else {
            result.warnings.push(Warning::new("unknown_id", None, format!("no face {id}")));
            return result;
        };
        result.hits = e
            .similar(src as usize, &ctx)
            .into_iter()
            .enumerate()
            .map(|(k, (i, score))| Hit {
                id: e.st.fid(i),
                text: e.st.text(i).to_string(),
                score,
                rank: (ctx.offset + k) as u32,
                flags: flags_of(e, &e.st.nums().at(i), ctx.long_at),
                why: None,
            })
            .collect();
        result
    }
}

/// `Database` must stay shareable across threads.
const _: () = {
    const fn is<T: Send + Sync + Clone>() {}
    is::<Database>()
};
