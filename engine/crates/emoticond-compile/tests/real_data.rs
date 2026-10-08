//! Round trip over the real data, when it is present: compile today's
//! inputs into a .kmj (in a temp dir, never next to the export), open it,
//! and check search behaviour on it.
//!
//! The export (gitignored `work/engine/`) is found at `$EMOTICOND_TEST_REPO`
//! (a checkout holding `work/engine/`), else in the checkout this crate
//! lives in. The curated data (`data/`, `data/situations.json`)
//! always comes from this checkout: it is the source tree under test.
//! Without an export every test passes without checking anything (and says
//! so).

use emoticond::*;
use emoticond_compile::legacy::LegacyPaths;
use emoticond_compile::Params;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn export_dir() -> Option<PathBuf> {
    let candidates = std::env::var_os("EMOTICOND_TEST_REPO").map(PathBuf::from).into_iter().chain([checkout()]);
    candidates.into_iter().map(|r| r.join("work/engine")).find(|d| d.join("faces.jsonl").exists())
}

fn inputs() -> Option<LegacyPaths> {
    Some(LegacyPaths::for_repo(&checkout(), &export_dir()?))
}

/// The compiled bytes and the file they were written to (compiled once).
fn compiled() -> Option<&'static (Vec<u8>, PathBuf)> {
    static C: OnceLock<Option<(Vec<u8>, PathBuf)>> = OnceLock::new();
    C.get_or_init(|| {
        let p = inputs()?;
        let bytes = emoticond_compile::compile(&p.read(Params::default()).unwrap()).unwrap();
        // one fixed dir, replaced by rename: runs do not pile up copies
        let dir = std::env::temp_dir().join("emoticond-test-real-data");
        let file = dir.join("full.kmj");
        emoticond_compile::write_atomic(&file, &bytes).unwrap();
        Some((bytes, file))
    })
    .as_ref()
}

fn open_with(f: impl FnOnce(&mut OpenOptions)) -> Option<Database> {
    let (_, file) = compiled()?;
    let mut o = OpenOptions::file(file);
    f(&mut o);
    Some(Database::open(o).unwrap())
}

fn db() -> Option<&'static Database> {
    static DB: OnceLock<Option<Database>> = OnceLock::new();
    let d = DB.get_or_init(|| open_with(|_| {})).as_ref();
    if d.is_none() {
        eprintln!("no work/engine export found: skipping");
    }
    d
}

fn texts(r: &SearchResult) -> Vec<&str> {
    r.hits.iter().map(|h| h.text.as_str()).collect()
}

const SHRUG: &str = "¯\\_(ツ)_/¯";

#[test]
fn shrug_is_pinned_first_with_its_id() {
    let Some(db) = db() else { return };
    let r = db.search("shrug", &SearchOptions::legacy(10));
    let h = &r.hits[0];
    assert_eq!(h.text, SHRUG);
    assert_eq!(h.id.to_string(), "k2026da3e4989");
    assert!(h.flags.contains(Flags::PINNED));
    assert_eq!(h.rank, 0);
    assert_eq!(r.term_key.as_deref(), Some("shrug"));
    assert_eq!(db.get(h.id).unwrap().text, SHRUG);
    assert_eq!(db.find_text(SHRUG).unwrap().id, h.id);
    assert!(db.get(h.id).unwrap().canonical_for.contains(&"shrug".to_string()));
}

#[test]
fn deterministic_and_ordered() {
    let Some(db) = db() else { return };
    for q in ["happy", "a bit sad", "sa", "(ツ)", "hug", "shy proud"] {
        let o = SearchOptions::legacy(40);
        let a = db.search(q, &o);
        assert_eq!(a, db.search(q, &o), "{q}");
        for w in a.hits.windows(2) {
            assert!(w[0].score > w[1].score || (w[0].score == w[1].score && w[0].id < w[1].id), "{q}: order");
        }
    }
}

#[test]
fn paging_matches_one_long_page() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(20);
    let all = db.search("happy", &o);
    o.limit = 5;
    o.offset = 10;
    let page = db.search("happy", &o);
    assert_eq!(texts(&page), texts(&all)[10..15]);
    assert_eq!(page.hits[0].rank, 10);
}

#[test]
fn strict_removes_explicit_faces_and_asking_does_not_unlock() {
    let Some(db) = db() else { return };
    for q in ["lewd", "nsfw horny", "butt"] {
        let strict = db.search(q, &SearchOptions::default());
        assert!(strict.hits.iter().all(|h| !h.flags.contains(Flags::EXPLICIT)), "{q}");
        assert_eq!(strict.safety, Safety::Strict);
    }
    let moderate = db.search("lewd", &SearchOptions::legacy(40));
    assert!(moderate.hits.iter().any(|h| h.flags.contains(Flags::EXPLICIT)));
    let mut o = SearchOptions::default();
    o.explain = Explain::Reading;
    let line = db.search("lewd", &o).reading.unwrap().line;
    assert!(line.contains("strict"), "{line}");
}

#[test]
fn styles_and_hard_filters() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(40);
    o.styles.lenny = StyleMode::Hide;
    o.styles.crude = StyleMode::Hide;
    o.max_len = Some(8);
    o.faces_only = true;
    o.min_quality = Some(5.0);
    for q in ["lenny", "happy", "middle finger"] {
        for h in &db.search(q, &o).hits {
            let e = db.get(h.id).unwrap();
            assert!(!h.flags.contains(Flags::LENNY) && !h.flags.contains(Flags::CRUDE), "{q} {}", h.text);
            assert!(e.text.chars().count() <= 8 && e.attrs.face >= 0.5 && e.quality >= 5.0, "{q} {}", h.text);
        }
    }
    let mut o = SearchOptions::legacy(40);
    o.styles.long = StyleMode::Hide;
    assert!(db.search("hug", &o).hits.iter().all(|h| h.text.chars().count() <= 14));
}

#[test]
fn exclude_and_blocklist() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(10);
    let first = db.search("shrug", &o).hits[0].id;
    o.exclude.insert(first);
    assert!(db.search("shrug", &o).hits.iter().all(|h| h.id != first));
    let blocked = open_with(|oo| {
        oo.blocklist.insert(first);
    })
    .unwrap();
    assert!(blocked.search("shrug", &SearchOptions::legacy(10)).hits.iter().all(|h| h.id != first));
    assert!(blocked.search("_(ツ)_", &SearchOptions::legacy(500)).hits.iter().all(|h| h.id != first));
}

#[test]
fn pinned_off_drops_the_canonical_lead() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(10);
    o.pinned = false;
    let r = db.search("shrug", &o);
    assert!(r.hits.iter().all(|h| !h.flags.contains(Flags::PINNED)));
    assert!(r.hits[0].score < 10.0);
}

#[test]
fn usage_lifts_faces_but_never_past_a_pin() {
    let Some(db) = db() else { return };
    let base = db.search("happy", &SearchOptions::legacy(40));
    let target = base.hits[25].id;
    let mut o = SearchOptions::legacy(40);
    o.usage.by_term.entry("happy".into()).or_default().insert(target, 1.0);
    o.usage.global.insert(target, 1.0);
    o.usage_weight = UsageWeight::High;
    let r = db.search("happy", &o);
    let at = r.hits.iter().position(|h| h.id == target).unwrap();
    assert!(at < 25, "moved from 25 to {at}");
    let pins = base.hits.iter().filter(|h| h.flags.contains(Flags::PINNED)).count();
    assert!(at >= pins, "usage must not beat a pinned face");
    o.usage_weight = UsageWeight::Off;
    assert_eq!(texts(&db.search("happy", &o)), texts(&base));
}

#[test]
fn variety_is_seeded() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(40);
    o.variety = 1.0;
    o.seed = 7;
    let a = db.search("sad", &o);
    assert_eq!(a, db.search("sad", &o));
    o.seed = 8;
    assert_ne!(texts(&a), texts(&db.search("sad", &o)));
    assert_eq!(a.hits[0].text, db.search("sad", &SearchOptions::legacy(1)).hits[0].text, "a pin stays first");
}

#[test]
fn dedupe_levels() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(40);
    let normal = db.search("cat", &o);
    o.dedupe = Dedupe::Off;
    let off = db.search("cat", &o);
    o.dedupe = Dedupe::Strong;
    let strong = db.search("cat", &o);
    assert_eq!(off.hits.len(), 40);
    assert_ne!(texts(&off), texts(&normal));
    assert_ne!(texts(&strong), texts(&normal));
}

#[test]
fn toggles_correct_complete_glyph() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(10);
    assert_eq!(db.search("shruging", &o).corrected.as_deref(), Some("shrug"));
    o.correct = false;
    assert_eq!(db.search("shruging", &o).corrected, None);
    let o = SearchOptions::legacy(10);
    assert!(matches!(db.read("embar", &o).mode, ReadMode::Partial { .. }));
    let mut o2 = o.clone();
    o2.complete_partial = false;
    assert!(!matches!(db.read("embar", &o2).mode, ReadMode::Partial { .. }));
    assert_eq!(db.read("(ツ)", &o).mode, ReadMode::Glyph);
    assert!(db.search("(ツ)", &o).hits.iter().all(|h| h.text.contains("(ツ)")));
    let mut o3 = o.clone();
    o3.glyph_search = GlyphSearch::Off;
    assert_ne!(db.read("(ツ)", &o3).mode, ReadMode::Glyph);
}

#[test]
fn figures_and_intensity() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(20);
    o.figures = Figures::Pair;
    let pair = db.search("happy", &o);
    o.figures = Figures::Single;
    let single = db.search("happy", &o);
    let multi = |r: &SearchResult| r.hits.iter().filter(|h| h.flags.contains(Flags::MULTI)).count();
    assert!(multi(&pair) > multi(&single));
    let mut o = SearchOptions::legacy(10);
    o.intensity = Intensity::High;
    o.explain = Explain::Reading;
    assert!(db.search("angry", &o).reading.unwrap().line.starts_with("very angry"));
}

#[test]
fn emotions_target_and_filters() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(20);
    o.emotions.target.insert("sad".into(), 0.3);
    o.emotions.max.insert("angry".into(), 0.2);
    o.emotions.min.insert("nope".into(), 0.5);
    let r = db.search("", &o);
    assert_eq!(r.hits.len(), 20, "an empty query with a target ranks by it");
    assert!(r.warnings.iter().any(|w| w.code == "unknown_emotion"));
    for h in &r.hits {
        let e = db.get(h.id).unwrap();
        assert!(e.emotions.iter().find(|(n, _)| n == "angry").unwrap().1 <= 0.2);
    }
    o.emotions.min.insert("angry".into(), 0.5);
    let r = db.search("happy", &o);
    assert!(r.hits.is_empty() && r.warnings.iter().any(|w| w.code == "contradiction"));
}

#[test]
fn policy_is_applied_on_every_query() {
    let Some(_) = db() else { return };
    let db = open_with(|o| o.policy.max_safety = Some(Safety::Strict)).unwrap();
    let r = db.search("lewd", &SearchOptions::legacy(40));
    assert_eq!(r.clamped, ["safety"]);
    assert_eq!(r.safety, Safety::Strict);
    assert!(r.hits.iter().all(|h| !h.flags.contains(Flags::EXPLICIT)));
}

#[test]
fn explain_full_and_reports() {
    let Some(db) = db() else { return };
    let mut o = SearchOptions::legacy(5);
    o.explain = Explain::Full;
    let r = db.search("very angry", &o);
    let reading = r.reading.clone().unwrap();
    assert!(reading.line.starts_with("very angry"), "{}", reading.line);
    assert_eq!(reading.terms[0].modifier, Some(Modifier::High));
    assert!(reading.debug.unwrap().starts_with("terms:"));
    let why = r.hits[0].why.clone().unwrap();
    assert_eq!(why.total, r.hits[0].score);
    let rep = Report::for_face(&r, "very angry", &r.hits[1], Reason::NoFit).stamp("id", 1, 1).build().unwrap();
    assert_eq!(rep.shown.len(), 5);
    assert_eq!(rep.reading.as_deref(), Some(r.reading.as_ref().unwrap().line.as_str()));
}

#[test]
fn browse_complete_similar_entries() {
    let Some(db) = db() else { return };
    let b = db.search("", &SearchOptions::legacy(10));
    assert_eq!(b.hits.len(), 10);
    assert!(b.hits.iter().all(|h| h.flags.contains(Flags::PINNED)));
    let c = db.complete("hap", 5);
    assert!(c.iter().any(|c| c.text == "happy"));
    let s = db.similar(FaceId::of_text(SHRUG), &SearchOptions::legacy(10));
    assert_eq!(s.hits.len(), 10);
    assert!(s.hits.iter().all(|h| h.text != SHRUG));
    let info = db.info();
    assert_eq!(db.entries().count(), info.faces);
    assert_eq!(info.emotions.len(), 19);
    let mut prev = None;
    for e in db.entries().take(1000) {
        assert!(prev < Some(e.id));
        prev = Some(e.id);
    }
}

#[test]
fn stranger_defaults_hide_crude() {
    let Some(db) = db() else { return };
    for q in ["middle finger", "fuck you", "rude"] {
        assert!(db.search(q, &SearchOptions::default()).hits.iter().all(|h| !h.flags.contains(Flags::CRUDE)), "{q}");
    }
}

#[test]
fn open_errors_are_clear() {
    let e = Database::open(OpenOptions::new("/nonexistent/emoticond")).unwrap_err();
    assert!(matches!(e, OpenError::NotFound { .. }), "{e}");
    let e = Database::open(OpenOptions::file("/nonexistent/full.kmj")).unwrap_err();
    assert!(matches!(e, OpenError::NotFound { .. }), "{e}");
    // an old EMOIDX index is refused clearly, not misread
    if let Some(old) = export_dir().map(|d| d.join("engine.idx")).filter(|p| p.exists()) {
        let e = Database::open(OpenOptions::file(old)).unwrap_err();
        assert!(matches!(e, OpenError::Incompatible { .. }), "{e}");
    }
}

#[test]
fn round_trip_matches_the_inputs() {
    let Some(db) = db() else { return };
    let src = inputs().unwrap().read(Params::default()).unwrap();
    let i = db.info();
    assert_eq!(i.faces, src.faces.len());
    assert_eq!(i.terms, src.vocab.len());
    assert_eq!(i.phrases, src.phrases.len());
    assert_eq!((i.format.as_str(), i.dataset.as_str(), i.licence.as_str()), ("3.1", "full", "CC-BY-4.0"));
    assert_eq!(i.emotions, src.emotions);
    assert!(db.licence().contains("CC BY 4.0") && !db.attribution().is_empty());
    assert_eq!(db.manifest().sexual_at, 1.5);
    db.verify().unwrap();
    // every face is there, by id and by text, with its numbers
    for f in src.faces.iter().step_by(97) {
        let e = db.find_text(&f.text).unwrap();
        assert_eq!(e.id, FaceId::of_text(&f.text));
        assert_eq!(db.get(e.id).unwrap().text, f.text);
        assert_eq!(e.quality, f.quality);
        assert_eq!(e.attrs.suggestive, f.suggestive);
    }
}

#[test]
fn compile_is_deterministic() {
    let Some((bytes, _)) = compiled() else { return };
    let again = emoticond_compile::compile(&inputs().unwrap().read(Params::default()).unwrap()).unwrap();
    assert!(again == *bytes, "two compiles of the same inputs differ");
}

/// The lossless encodings (the default) answer exactly as format 3.0.
#[test]
fn compact_answers_as_legacy() {
    let Some(db) = db() else { return };
    let p = Params { encoding: emoticond_compile::Encoding::legacy(), ..Params::default() };
    let legacy = Database::from_bytes(emoticond_compile::compile(&inputs().unwrap().read(p).unwrap()).unwrap(), OpenOptions::default()).unwrap();
    assert!(legacy.info().bytes > 2 * db.info().bytes, "{} vs {}", legacy.info().bytes, db.info().bytes);
    let mut full = SearchOptions::legacy(40);
    full.explain = Explain::Full;
    for q in ["shrug", "a bit sad", "sa", "(ツ)", "very angry", "movie night", "", "kind of tired", "cat", "happy cat", "not happy", "lenny", "table flip", "嬉しい"] {
        assert_eq!(db.search(q, &full), legacy.search(q, &full), "{q}");
        assert_eq!(db.search(q, &SearchOptions::default()), legacy.search(q, &SearchOptions::default()), "{q}");
    }
    let id = db.search("shrug", &full).hits[0].id;
    assert_eq!(db.similar(id, &full), legacy.similar(id, &full));
    assert_eq!(db.entries().step_by(101).collect::<Vec<_>>(), legacy.entries().step_by(101).collect::<Vec<_>>());
}

/// core and lite from the selection list (`work/sets/private/core.txt`, if
/// present): deterministic, the selected faces, every set answers.
#[test]
fn sets_compile_deterministically() {
    let Some(p) = inputs() else { return };
    let sel = p.engine_dir.join("../sets/private/core.txt");
    if !sel.exists() {
        eprintln!("no {}: skipping", sel.display());
        return;
    }
    let ids = emoticond_compile::legacy::read_selection(&sel).unwrap();
    let base = p.read(Params::default()).unwrap();
    for (set, k) in [("core", None), ("lite", Some(32))] {
        let build = || {
            let mut s = base.clone();
            s.params.dataset = set.into();
            let kept = s.retain_faces(|f| ids.contains(&FaceId::of_text(&f.text).as_u64()));
            if let Some(k) = k {
                s.truncate_neighbours(k);
            }
            (kept, emoticond_compile::compile(&s).unwrap())
        };
        let (kept, a) = build();
        assert!(a == build().1, "{set}: two compiles differ");
        let db = Database::from_bytes(a, OpenOptions::default()).unwrap();
        assert_eq!((db.info().faces, db.info().dataset.as_str()), (kept, set));
        assert_eq!(db.manifest().dense_k, k.unwrap_or(base.dense_k));
        assert_eq!(db.search("shrug", &SearchOptions::legacy(5)).hits[0].text, SHRUG, "{set}");
    }
}

#[test]
fn from_bytes_matches_the_mapped_file() {
    let (Some(db), Some((bytes, _))) = (db(), compiled()) else { return };
    // an owned, deliberately misaligned copy: from_bytes copies it once
    let mut v = vec![0u8; bytes.len() + 1];
    v[1..].copy_from_slice(bytes);
    let mem = Database::from_bytes(v[1..].to_vec(), OpenOptions::default()).unwrap();
    for q in ["shrug", "a bit sad", "sa", "(ツ)", "very angry", "movie night", "", "kind of tired"] {
        let o = SearchOptions::legacy(40);
        assert_eq!(db.search(q, &o), mem.search(q, &o), "{q}");
    }
    assert_eq!(mem.info().path, "");
}

#[test]
fn data_dirs_find_the_file_and_fall_back_between_sets() {
    let Some((_, file)) = compiled() else { return };
    let dir = file.parent().unwrap();
    let mut o = OpenOptions::new(dir);
    o.dataset = Dataset::Core;
    let db = Database::open(o).unwrap();
    assert!(db.warnings().iter().any(|w| w.code == "dataset_fallback"), "core asked, full found");
    let mut o = OpenOptions::new(dir);
    o.dataset = Dataset::Full;
    assert!(Database::open(o).unwrap().warnings().is_empty());
}

#[test]
fn open_is_fast() {
    let Some((_, file)) = compiled() else { return };
    let t = std::time::Instant::now();
    let db = Database::open(OpenOptions::file(file)).unwrap();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("open: {ms:.3} ms");
    assert!(db.info().faces > 0);
    assert!(ms < 50.0, "open took {ms} ms");
}

// ---- overlays over the real data (step 8) ---------------------------------

fn with_overlays(o: Overlay) -> Option<Database> {
    open_with(|opts| opts.overlays = vec![OverlaySource::Overlay(o)])
}

#[test]
fn overlay_user_pin_beats_shipped_pick_and_usage() {
    let Some(db) = db() else { return };
    let q = SearchOptions::legacy(20);
    let shipped = db.search("shrug", &q);
    // the user's own shrug: a face the shipped data ranks low
    let mine = shipped.hits[15].clone();
    let mut o = Overlay::new();
    o.pin("shrug", mine.id, 1);
    let user = with_overlays(o).unwrap();
    let mut q = SearchOptions::legacy(20);
    q.usage_weight = UsageWeight::High;
    q.usage.by_term.entry("shrug".into()).or_default().insert(shipped.hits[5].id, 1.0);
    q.usage.global.insert(shipped.hits[5].id, 1.0);
    let r = user.search("shrug", &q);
    assert_eq!(r.hits[0].id, mine.id);
    assert!(r.hits[0].flags.contains(Flags::USER_PIN | Flags::PINNED));
    assert_eq!(r.hits[1].text, SHRUG, "the shipped pick follows the user's");
    // usage lifts the used face, but never above a pin
    let used = texts(&r).iter().position(|t| *t == shipped.hits[5].text).unwrap();
    let unused = texts(&user.search("shrug", &SearchOptions::legacy(20))).iter().position(|t| *t == shipped.hits[5].text).unwrap();
    let pins = r.hits.iter().filter(|h| h.flags.contains(Flags::PINNED)).count();
    assert!(used < unused && used >= pins, "{used} {unused} {pins}");
}

#[test]
fn overlay_demote_from_state_lowers_that_face_for_that_term_only() {
    let Some(db) = db() else { return };
    let q = SearchOptions::legacy(40);
    let happy = db.search("happy", &q);
    // an unpinned face high in the list
    let victim = happy.hits.iter().find(|h| !h.flags.contains(Flags::PINNED)).unwrap().clone();
    // what emoticond-state writes for "doesn't fit"
    let row = format!("{{\"term\":\"happy\",\"id\":\"{}\",\"boost\":-3,\"why\":\"doesn't fit (report)\"}}\n", victim.id.hex12());
    let user = open_with(|o| o.overlays = vec![OverlaySource::Inline { name: "boosts.jsonl".into(), text: row }]).unwrap();
    assert!(user.warnings().is_empty(), "{:?}", user.warnings());
    let after = user.search("happy", &q);
    let was = texts(&happy).iter().position(|t| *t == victim.text).unwrap();
    let now = texts(&after).iter().position(|t| *t == victim.text);
    assert!(now.is_none_or(|n| n > was), "{was} -> {now:?}");
    for other in ["sad", "shrug", "joy"] {
        assert_eq!(db.search(other, &q), user.search(other, &q), "{other}");
    }
    // a demote of the pinned face unpins it for that term
    let row = "{\"term\":\"shrug\",\"id\":\"k2026da3e4989\",\"boost\":-3}\n".to_string();
    user.reload_overlays(&[OverlaySource::Inline { name: "boosts.jsonl".into(), text: row }]);
    assert_ne!(user.search("shrug", &q).hits[0].text, SHRUG);
    assert_eq!(user.search("idk", &q).hits[0].text, SHRUG, "other terms keep it");
}

#[test]
fn overlay_user_phrase_resolves_and_is_marked() {
    let Some(db) = db() else { return };
    let mut o = Overlay::new();
    o.phrase(OverlayPhrase::new("zoomies", &[("excited", 0.7), ("playful", 0.3)]).spellings(["the zoomies", "zoomy"]).words(["running"]));
    o.phrase(OverlayPhrase::alias("shruggo", "shrug"));
    let user = with_overlays(o).unwrap();
    let mut q = SearchOptions::legacy(20);
    q.explain = Explain::Reading;
    let r = user.search("got the zoomies", &q);
    assert_eq!(r.term_key.as_deref(), Some("zoomies"));
    assert_eq!(r.term_source, TermSource::Overlay);
    assert!(r.reading.as_ref().unwrap().line.contains("yours"), "{:?}", r.reading);
    assert!(!r.hits.is_empty());
    assert!(db.search("shruggo", &q).term_key.as_deref() != Some("shrug") || db.search("shruggo", &q).corrected.is_some());
    let r = user.search("shruggo", &q);
    assert_eq!(r.corrected, None);
    assert_eq!((r.hits[0].text.as_str(), r.term_key.as_deref()), (SHRUG, Some("shrug")));
    assert_eq!(db.search("shrug", &SearchOptions::legacy(20)).term_source, TermSource::Shipped);
}

#[test]
fn overlay_hide_and_empty_overlays_change_nothing_else() {
    let Some(db) = db() else { return };
    let mut o = Overlay::new();
    o.hide(FaceId::of_text(SHRUG));
    let user = with_overlays(o).unwrap();
    assert!(user.search("shrug", &SearchOptions::legacy(40)).hits.iter().all(|h| h.text != SHRUG));
    // an empty overlay is exactly no overlay
    let empty = with_overlays(Overlay::new()).unwrap();
    for q in ["happy", "a bit sad", "sa", "(ツ)", "hug", "shy proud", "idk", "movie night", ""] {
        let o = SearchOptions::legacy(40);
        assert_eq!(db.search(q, &o), empty.search(q, &o), "{q}");
    }
}
