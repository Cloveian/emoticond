//! User overlays over the tiny fixture: pins, phrases, boosts, hidden
//! faces, precedence between layers, warnings, and atomic swaps under
//! concurrent searches. Typed overlays need no file system; the JSONL
//! tests need the `fs` feature.

use emoticond::*;
use std::collections::BTreeSet;

static TINY: &[u8] = emoticond::include_data!("fixtures/tiny.kmj");

const SHRUG: &str = "¯\\_(ツ)_/¯";
const SMILE: &str = "(^‿^)";
const FLOWER: &str = "(◕‿◕✿)";
const BEAR: &str = "ʕ•ᴥ•ʔ";
const HUG: &str = "(づ｡◕‿‿◕｡)づ";
const CRY: &str = "(╥﹏╥)";
const SHY: &str = "(*/ω＼*)";

/// The face's score for a query (legacy options, 14 results), if listed.
fn score(db: &Database, q: &str, face: &str) -> Option<f32> {
    db.search(q, &SearchOptions::legacy(14)).hits.iter().find(|h| h.text == face).map(|h| h.score)
}

fn id(t: &str) -> FaceId {
    FaceId::of_text(t)
}

fn db_with(layers: Vec<Overlay>) -> Database {
    let mut o = OpenOptions::default();
    o.overlays = layers.into_iter().map(OverlaySource::Overlay).collect();
    Database::from_bytes(TINY, o).unwrap()
}

fn plain() -> Database {
    Database::from_bytes(TINY, OpenOptions::default()).unwrap()
}

fn texts(r: &SearchResult) -> Vec<&str> {
    r.hits.iter().map(|h| h.text.as_str()).collect()
}

fn opts() -> SearchOptions {
    let mut o = SearchOptions::legacy(10);
    o.explain = Explain::Reading;
    o
}

#[test]
fn user_pin_beats_shipped_canonical_and_usage() {
    let mut o = Overlay::new();
    o.pin("Happy", id(BEAR), 1);
    let db = db_with(vec![o]);
    let mut q = opts();
    q.usage_weight = UsageWeight::High;
    q.usage.global.insert(id(SHY), 1.0);
    q.usage.by_term.entry("happy".into()).or_default().insert(id(SHY), 1.0);
    let r = db.search("happy", &q);
    assert_eq!(&texts(&r)[..3], [BEAR, SMILE, FLOWER], "user pin, then the shipped picks");
    assert!(r.hits[0].flags.contains(Flags::PINNED | Flags::USER_PIN));
    assert!(r.hits[1].flags.contains(Flags::PINNED) && !r.hits[1].flags.contains(Flags::USER_PIN));
    assert!(r.reading.as_ref().unwrap().line.contains("your pinned faces first"));
    // usage lifts the shy face, but only up to just below the pins
    assert_eq!(texts(&r)[3], SHY);
    assert!(r.hits[2].score > r.hits[3].score + 5.0);
    assert_eq!(r.term_source, TermSource::Shipped, "the concept itself is shipped");
    assert_eq!(db.get(id(BEAR)).unwrap().canonical_for, ["happy"]);
    // the empty query shows the user's pinned terms first
    let b = db.search("", &SearchOptions::legacy(3));
    assert_eq!(b.hits[0].text, BEAR);
    assert!(b.hits[0].flags.contains(Flags::USER_PIN));
    // without the overlay nothing of this
    let r = plain().search("happy", &q);
    assert_eq!(r.hits[0].text, SMILE);
    assert!(r.hits.iter().all(|h| !h.flags.contains(Flags::USER_PIN)));
}

#[test]
fn later_layers_win_and_clear_drops_what_is_below() {
    let mut state = Overlay::new();
    state.pin("happy", id(BEAR), 1);
    let mut user = Overlay::new();
    user.pin("happy", id(HUG), 1).pin("happy", id(CRY), 2);
    let db = db_with(vec![state.clone(), user.clone()]);
    assert_eq!(&texts(&db.search("happy", &opts()))[..3], [HUG, CRY, BEAR], "the later layer leads; three at most");

    let mut clear = Overlay::new();
    clear.clear("happy");
    let db = db_with(vec![state, clear.clone()]);
    let r = db.search("happy", &opts());
    assert!(r.hits.iter().all(|h| !h.flags.contains(Flags::PINNED)), "{:?}", texts(&r));
    assert!(!r.reading.as_ref().unwrap().line.contains("pinned"));
    // a clear and new pins in one layer: only those pins
    clear.pin("happy", id(CRY), 1);
    let r = db_with(vec![clear]).search("happy", &opts());
    assert_eq!(r.hits[0].text, CRY);
    assert_eq!(r.hits.iter().filter(|h| h.flags.contains(Flags::PINNED)).count(), 1);
}

#[test]
fn user_phrase_resolves_and_is_marked() {
    let mut o = Overlay::new();
    o.phrase(OverlayPhrase::new("blep", &[("playful", 0.5), ("happy", 0.5)]).spellings(["bleppy"]).words(["cute"]));
    o.phrase(OverlayPhrase::alias("meh", "shrug"));
    o.phrase(OverlayPhrase::alias("whatever", "over it"));
    let db = db_with(vec![o]);

    let r = db.search("bleppy", &opts());
    assert_eq!(r.term_key.as_deref(), Some("blep"));
    assert_eq!(r.term_source, TermSource::Overlay);
    assert_eq!(r.hits[0].text, FLOWER, "happy and playful, tagged cute: {:?}", texts(&r));
    let reading = r.reading.unwrap();
    assert!(reading.line.starts_with("blep (") && reading.line.contains("yours"), "{}", reading.line);
    assert!(reading.terms[0].user);

    // an alias of a vocab term reads as that concept, with its pins
    let r = db.search("meh", &opts());
    assert_eq!(r.term_key.as_deref(), Some("shrug"));
    assert_eq!(r.term_source, TermSource::Overlay);
    assert_eq!(r.hits[0].text, SHRUG);
    assert!(r.hits[0].flags.contains(Flags::PINNED));
    // an alias of a shipped phrase
    let r = db.search("whatever", &opts());
    assert_eq!(r.term_key.as_deref(), Some("over it"));

    // without the overlay these words mean nothing
    let p = plain();
    assert_ne!(p.search("bleppy", &opts()).term_key.as_deref(), Some("blep"));
    // typeahead offers the user's spellings first
    let c = db.complete("bl", 5);
    assert_eq!((c[0].text.as_str(), c[0].source), ("blep", CompletionSource::Overlay));
    // and correction knows them
    assert_eq!(db.search("blepy", &SearchOptions::legacy(5)).corrected.as_deref(), Some("blep"));
}

#[test]
fn user_phrase_overrides_shipped_spelling_and_vocab_word() {
    let mut o = Overlay::new();
    o.phrase(OverlayPhrase::new("uwu", &[("sad", 1.0)]));
    o.phrase(OverlayPhrase::new("angry", &[("sleepy", 1.0)]));
    let db = db_with(vec![o]);
    let r = db.search("uwu", &opts());
    assert_eq!(r.hits[0].text, CRY, "{:?}", texts(&r));
    assert!(r.reading.as_ref().unwrap().line.contains("yours"));
    let r = db.search("angry", &opts());
    assert_eq!(r.term_source, TermSource::Overlay);
    assert!(r.reading.as_ref().unwrap().terms[0].user);
    // shipped: uwu is happy and shy
    assert_ne!(plain().search("uwu", &opts()).hits[0].text, CRY);
}

#[test]
fn demote_lowers_that_face_for_that_term_only() {
    let base = plain();
    let mut o = Overlay::new();
    o.boost("happy", id(BEAR), -3.0);
    let db = db_with(vec![o]);
    // the score moved by exactly the boost change: shipped +3 -> overlay -3
    let (s0, s1) = (score(&base, "happy", BEAR).unwrap(), score(&db, "happy", BEAR).unwrap());
    assert!((s0 - s1 - 6.0 * 0.08).abs() < 1e-5, "{s0} {s1}");
    // other terms, and other faces for this term, are untouched
    assert_eq!(base.search("bear", &opts()), db.search("bear", &opts()));
    assert_eq!(score(&base, "happy", SHY), score(&db, "happy", SHY));
    // (rank changes on real data: emoticond-compile's real_data tests)

    // a demote unpins a shipped pick for that term, never a user pin
    let mut o = Overlay::new();
    o.boost("happy", id(SMILE), -3.0).boost("shrug", id(BEAR), -3.0);
    let db = db_with(vec![o.clone()]);
    let r = db.search("happy", &opts());
    assert_eq!(r.hits[0].text, FLOWER);
    assert!(r.hits.iter().all(|h| h.text != SMILE || !h.flags.contains(Flags::PINNED)));
    assert_eq!(db.search("shrug", &opts()).hits[0].text, SHRUG);
    o.pin("happy", id(SMILE), 1);
    assert_eq!(db_with(vec![o]).search("happy", &opts()).hits[0].text, SMILE);
}

#[test]
fn hidden_faces_are_never_returned_and_join_the_blocklist() {
    let mut o = Overlay::new();
    o.hide(id(SHRUG));
    let db = db_with(vec![o]);
    assert!(db.search("shrug", &opts()).hits.iter().all(|h| h.text != SHRUG));
    assert!(db.search("", &SearchOptions::legacy(20)).hits.iter().all(|h| h.text != SHRUG));
    db.set_blocklist([id(SMILE)].into_iter().collect());
    let r = db.search("happy", &opts());
    assert!(r.hits.iter().all(|h| h.text != SMILE));
    assert!(db.search("shrug", &opts()).hits.iter().all(|h| h.text != SHRUG), "the overlay still hides");
    assert_eq!(db.blocklist(), [id(SMILE)].into_iter().collect::<BTreeSet<_>>());
    // dropping the overlay keeps the blocklist
    assert!(db.reload_overlays(&[]).is_empty());
    assert_eq!(db.search("shrug", &opts()).hits[0].text, SHRUG);
    assert!(db.search("happy", &opts()).hits.iter().all(|h| h.text != SMILE));
}

#[test]
fn unknown_faces_and_alias_targets_are_warnings() {
    let mut o = Overlay::new();
    o.pin("happy", id("not in the data"), 1)
        .boost("happy", id("nor this"), 1.0)
        .phrase(OverlayPhrase::alias("x", "no such concept"))
        .phrase(OverlayPhrase::new("y", &[("glee", 1.0)]));
    let db = db_with(vec![o.clone()]);
    let codes: Vec<&str> = db.warnings().iter().map(|w| w.code.as_ref()).collect();
    assert_eq!(codes, ["overlay_unknown_face", "overlay_unknown_face", "overlay_unknown_alias", "overlay_unknown_emotion", "overlay_bad_line"]);
    assert_eq!(db.search("happy", &opts()).hits[0].text, SMILE, "the shipped picks stand");
    let w = plain().reload_overlays(&[OverlaySource::Overlay(o)]);
    assert_eq!(w.len(), 5);
}

#[test]
fn set_defaults_swaps() {
    let db = plain();
    assert_eq!(db.defaults(), SearchOptions::default());
    let d = SearchOptions::legacy(7);
    let clone = db.clone();
    db.set_defaults(d.clone());
    assert_eq!(clone.defaults(), d, "clones share the swap");
}

#[test]
fn reload_is_atomic_under_concurrent_searches() {
    let db = plain();
    let mut pin = Overlay::new();
    pin.pin("happy", id(BEAR), 1).pin("happy", id(CRY), 2).hide(id(FLOWER)).boost("happy", id(HUG), 3.0);
    let with = [OverlaySource::Overlay(pin)];
    let q = SearchOptions::legacy(10);
    let none = db.search("happy", &q);
    db.reload_overlays(&with);
    let some = db.search("happy", &q);
    assert_ne!(none, some);
    db.reload_overlays(&[]);
    let blocked = {
        db.set_blocklist([id(SMILE)].into_iter().collect());
        let r = db.search("happy", &q);
        db.set_blocklist(BTreeSet::new());
        r
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..4 {
            let db = db.clone();
            let (none, some, blocked, q, stop) = (&none, &some, &blocked, &q, &stop);
            s.spawn(move || {
                let mut n = 0usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) || n == 0 {
                    let r = db.search("happy", q);
                    // one consistent snapshot: exactly one of the states set,
                    // never pins from one and hidden faces from another
                    assert!(&r == none || &r == some || &r == blocked, "mixed state: {:?}", texts(&r));
                    n += 1;
                }
            });
        }
        for k in 0..400 {
            if k % 2 == 0 {
                db.reload_overlays(&with);
            } else {
                db.reload_overlays(&[]);
            }
            if k % 50 == 25 {
                db.set_blocklist([id(SMILE)].into_iter().collect());
                db.set_blocklist(BTreeSet::new());
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    assert!(blocked.hits.iter().all(|h| h.text != SMILE));
    db.reload_overlays(&with);
    assert_eq!(db.search("happy", &q), some);
}

#[cfg(feature = "fs")]
mod files {
    use super::*;
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("emoticond-overlay-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, text).unwrap();
            p
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn state_and_config_dirs_in_order_with_warnings() {
        // machine-written (emoticond-state's exact rows), then hand-written
        let state = Dir::new("state");
        state.write(
            "boosts.jsonl",
            &format!("{{\"term\":\"happy\",\"id\":\"{}\",\"boost\":-3,\"why\":\"doesn't fit (report)\"}}\n", id(BEAR).hex12()),
        );
        let user = Dir::new("user");
        user.write("canonical.jsonl", &format!("{{\"term\":\"shrug\",\"text\":\"{}\",\"rank\":1}}\nnot json\n", HUG.replace('\\', "\\\\")));
        user.write("phrases.jsonl", "{\"key\":\"blep\",\"match\":[\"bleppy\"],\"p\":{\"playful\":1,\"happy\":1}}\n{\"key\":\"meh\",\"alias_of\":\"shrug\"}\n");
        user.write("blocklist.txt", &format!("# mine\n{CRY}\nk2026da3e4989x\n"));
        let sources = [OverlaySource::Path(state.0.clone()), OverlaySource::Path(user.0.clone())];
        let mut o = OpenOptions::default();
        o.overlays = sources.to_vec();
        let db = Database::from_bytes(TINY, o).unwrap();
        let codes: Vec<&str> = db.warnings().iter().map(|w| w.code.as_ref()).collect();
        assert_eq!(codes, ["overlay_bad_line", "overlay_bad_line"], "{:?}", db.warnings());
        assert!(db.warnings()[0].message.contains("canonical.jsonl:2"));

        let r = db.search("meh", &opts());
        assert_eq!(r.hits[0].text, HUG, "the user's pin for shrug, reached through the user's alias");
        assert!(r.hits[0].flags.contains(Flags::USER_PIN));
        assert_eq!(r.term_source, TermSource::Overlay);
        assert!(db.search("sad", &opts()).hits.iter().all(|h| h.text != CRY));
        let p = plain();
        assert!(score(&db, "happy", BEAR) < score(&p, "happy", BEAR));

        // the user's dir beats the state dir: a positive boost there wins
        user.write("boosts.jsonl", &format!("{{\"term\":\"happy\",\"id\":\"{}\",\"boost\":3}}\n", id(BEAR)));
        let w = db.reload_overlays(&sources);
        assert_eq!(w.len(), 2);
        assert_eq!(score(&db, "happy", BEAR), score(&p, "happy", BEAR));

        // what a front-end watches
        let watch: Vec<PathBuf> = sources.iter().flat_map(|s| s.watch_paths()).collect();
        assert_eq!(watch.len(), 8);
        assert_eq!(watch[0], state.0.join("canonical.jsonl"));
        assert_eq!(OverlaySource::Path(user.0.join("boosts.jsonl")).watch_paths(), [user.0.join("boosts.jsonl")]);
    }

    #[test]
    fn single_files_inline_text_and_missing_paths() {
        let d = Dir::new("files");
        let f = d.write("canonical_hand.jsonl", &format!("{{\"term\":\"happy\",\"text\":\"{BEAR}\"}}\n"));
        let db = plain();
        assert!(db.reload_overlays(&[OverlaySource::Path(f)]).is_empty());
        assert_eq!(db.search("happy", &opts()).hits[0].text, BEAR);
        let nope = d.write("nope.jsonl", "{}\n");
        let w = db.reload_overlays(&[
            OverlaySource::Inline { name: "blocklist.txt".into(), text: format!("{BEAR}\n") },
            OverlaySource::Path(nope),
            OverlaySource::Path(d.0.join("boosts.jsonl")),
        ]);
        let codes: Vec<&str> = w.iter().map(|w| w.code.as_ref()).collect();
        assert_eq!(codes, ["overlay_unknown_file", "overlay_missing"]);
        assert_eq!(w[1].level, Level::Info);
        assert_eq!(db.search("happy", &opts()).hits[0].text, SMILE, "the reload replaced the earlier overlays");
        assert!(db.search("happy", &opts()).hits.iter().all(|h| h.text != BEAR));
        // a missing directory is just empty
        assert!(db.reload_overlays(&[OverlaySource::Path(d.0.join("absent"))]).iter().all(|w| w.level == Level::Info));
    }
}

#[cfg(not(feature = "fs"))]
#[test]
fn files_need_fs_but_typed_overlays_work() {
    let w = plain().reload_overlays(&[OverlaySource::Path("/nowhere".into())]);
    assert_eq!(w[0].code, "unsupported");
}
