//! The committed tiny data file (`tests/fixtures/tiny.kmj`, built by
//! emoticond-compile's `tiny` test) opened from memory. Needs no file system:
//! this also runs with `--no-default-features`.

use emoticond::*;

static TINY: &[u8] = emoticond::include_data!("fixtures/tiny.kmj");

fn db() -> Database {
    Database::from_bytes(TINY, OpenOptions::default()).unwrap()
}

#[test]
fn opens_from_static_bytes() {
    assert_eq!(TINY.as_ptr() as usize % 8, 0, "include_data! aligns");
    let db = db();
    let i = db.info();
    assert_eq!((i.faces, i.terms, i.format.as_str(), i.dataset.as_str()), (14, 10, "3.1", "full"));
    assert_eq!((i.path.as_str(), i.data_dir.as_str()), ("", ""));
    assert_eq!(i.content_hash.len(), 64);
    assert_eq!(i.emotions.len(), 19);
    assert!(db.licence().starts_with("CC BY 4.0"));
    assert!(!db.attribution().is_empty());
    db.verify().unwrap();
    // a misaligned owned copy opens too (copied once)
    let mut v = vec![0u8; TINY.len() + 3];
    v[3..].copy_from_slice(TINY);
    let other = Database::from_bytes(v[3..].to_vec(), OpenOptions::default()).unwrap();
    assert_eq!(other.info(), i);
}

#[test]
fn searches_the_compiled_tables() {
    let db = db();
    let o = SearchOptions::legacy(10);
    let r = db.search("shrug", &o);
    assert_eq!(r.hits[0].text, "¯\\_(ツ)_/¯");
    assert!(r.hits[0].flags.contains(Flags::PINNED));
    assert_eq!(r.hits[0].id.to_string(), "k2026da3e4989");
    let r = db.search("happy", &o);
    assert_eq!(&r.hits[0].text, "(^‿^)");
    // the boosted bear outranks other unpinned happy faces
    let bear = r.hits.iter().position(|h| h.text == "ʕ•ᴥ•ʔ").unwrap();
    assert!(bear <= 3, "{:?}", r.hits.iter().map(|h| &h.text).collect::<Vec<_>>());
    let mut o = SearchOptions::legacy(10);
    o.explain = Explain::Reading;
    assert!(db.search("very angry", &o).reading.unwrap().line.starts_with("very angry"));
    assert!(db.search("kind of sad", &o).reading.unwrap().line.starts_with("a bit sad"));
    assert!(db.search("uwu", &o).reading.unwrap().line.contains("cute"));
    // correction against vocab and phrases
    assert_eq!(db.search("shurg", &SearchOptions::legacy(5)).corrected.as_deref(), Some("shrug"));
    assert!(db.complete("sh", 5).iter().any(|c| c.text == "shrug" && c.pinned));
}

#[test]
fn reads_typed_emoticons_typos_and_partial_words() {
    let db = db();
    let mut o = SearchOptions::legacy(10);
    o.explain = Explain::Reading;
    let line = |q: &str| db.search(q, &o).reading.unwrap().line;
    // a typed emoticon from the grammar table is its concept, reported as a correction
    let r = db.search(":(", &o);
    assert_eq!(r.corrected.as_deref(), Some("sad"));
    assert_eq!(r.hits[0].text, "(╥﹏╥)", "{:?}", r.hits.iter().map(|h| &h.text).collect::<Vec<_>>());
    assert!(line(":(").starts_with("\u{201c}:(\u{201d} as sad"), "{}", line(":("));
    assert_eq!(db.search("XD", &o).corrected.as_deref(), Some("laughing"), "case-insensitive");
    // punctuation after a word is not pasted glyphs
    assert!(line("happy!").starts_with("happy"), "{}", line("happy!"));
    assert!(line("sad?").starts_with("sad"), "{}", line("sad?"));
    // a stretched word is its plain form
    assert_eq!(db.search("happpy", &o).corrected.as_deref(), Some("happy"));
    assert_eq!(db.search("saaaad", &o).corrected.as_deref(), Some("sad"));
    // one edit is corrected inside a longer query too (five letters or more)
    assert_eq!(db.search("shurg happy", &o).corrected.as_deref(), Some("shrug"));
    assert!(line("shurg happy").starts_with("shrug"), "{}", line("shurg happy"));
    // an intensifier still modifies even when `very angry` could be a phrase
    assert!(line("very angry").starts_with("very angry"));
    // a trailing partial word completes, keeping the words before it
    let l = line("happy sh");
    assert!(l.starts_with("happy") && l.contains("completing: shrug"), "{l}");
    assert!(db.search("happy sh", &o).hits.iter().any(|h| h.text == "¯\\_(ツ)_/¯"));
}

#[test]
fn manifest_thresholds_drive_safety_and_flags() {
    let db = db();
    let kiss = db.find_text("(˘ε˘)").unwrap();
    assert!(kiss.flags.contains(Flags::EXPLICIT) && kiss.flags.contains(Flags::SUGGESTIVE));
    let lenny = db.find_text("( ͡° ͜ʖ ͡°)").unwrap();
    assert!(lenny.flags.contains(Flags::SUGGESTIVE) && !lenny.flags.contains(Flags::EXPLICIT));
    assert!(db.find_text("┌∩┐(◣_◢)┌∩┐").unwrap().flags.contains(Flags::CRUDE));
    let strict = db.search("kiss", &SearchOptions::default());
    assert!(strict.hits.iter().all(|h| h.text != "(˘ε˘)"));
    assert!(db.search("kiss", &SearchOptions::legacy(10)).hits.iter().any(|h| h.text == "(˘ε˘)"));
    assert!(db.search("angry", &SearchOptions::default()).hits.iter().all(|h| !h.flags.contains(Flags::CRUDE)));
}

#[test]
fn entries_are_in_id_order() {
    let db = db();
    let ids: Vec<FaceId> = db.entries().map(|e| e.id).collect();
    assert_eq!(ids.len(), 14);
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    for id in ids {
        assert_eq!(db.get(id).unwrap().id, id);
    }
}

#[test]
fn garbage_is_an_error() {
    assert!(matches!(Database::from_bytes(vec![1, 2, 3], OpenOptions::default()), Err(OpenError::Incompatible { .. })));
    assert!(matches!(Database::from_bytes(&TINY[..100], OpenOptions::default()), Err(OpenError::Corrupt { .. })));
}
