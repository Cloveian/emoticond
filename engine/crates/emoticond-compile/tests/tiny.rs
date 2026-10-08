//! The tiny fixture (`crates/emoticond/tests/fixtures/tiny.kmj`): built here
//! from sources written in code, checked for freshness, and used to check
//! that damaged files give errors (or degraded results), never a panic.
//!
//! After a deliberate format or fixture change, rebuild the committed copy
//! with `EMOTICOND_BLESS_FIXTURE=1 cargo test -p emoticond-compile --test tiny`.

use emoticond::data::format::{DIR_ENTRY_LEN, HEADER_LEN, SECTION_REQUIRED};
use emoticond::*;
use emoticond_compile::*;
use std::path::PathBuf;

mod common;
use common::tiny_sources;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../emoticond/tests/fixtures/tiny.kmj")
}

/// The fixture: the default encoding (compact, format 3.1).
fn tiny() -> Vec<u8> {
    compile(&tiny_sources()).unwrap()
}

fn tiny_as(e: Encoding) -> Vec<u8> {
    let mut s = tiny_sources();
    s.params.encoding = e;
    compile(&s).unwrap()
}

const ENCODINGS: [(&str, Encoding); 3] =
    [("legacy", Encoding::legacy()), ("compact", Encoding::compact()), ("small", Encoding::small())];

#[test]
fn committed_fixture_is_fresh() {
    let bytes = tiny();
    let path = fixture_path();
    if std::env::var_os("EMOTICOND_BLESS_FIXTURE").is_some() {
        write_atomic(&path, &bytes).unwrap();
    }
    let committed = std::fs::read(&path).unwrap_or_default();
    assert!(committed == bytes, "{} is stale: rebuild it with EMOTICOND_BLESS_FIXTURE=1", path.display());
}

#[test]
fn compile_is_deterministic() {
    assert_eq!(tiny(), tiny());
    for (name, e) in ENCODINGS {
        assert_eq!(tiny_as(e), tiny_as(e), "{name}");
    }
    assert_eq!(tiny_as(Encoding::compact()), tiny(), "compact is the default");
    assert_eq!(&tiny_as(Encoding::legacy())[8..12], &[3, 0, 0, 0], "legacy is format 3.0");
    assert_eq!(&tiny()[8..12], &[3, 0, 1, 0], "compact is format 3.1");
    // input order of boosts, situations, canonical rows and grammar
    // languages does not matter where the format sorts them
    let mut s = tiny_sources();
    s.canonical.reverse();
    s.grammar.reverse();
    let mut b = s.boosts.clone();
    b.rotate_right(1); // keeps the duplicates in order: the last (term, id) wins
    s.boosts = b;
    assert_eq!(compile(&s).unwrap(), tiny());
}

#[test]
fn bad_sources_are_refused() {
    let mut s = tiny_sources();
    s.faces.push(s.faces[0].clone());
    assert!(compile(&s).unwrap_err().to_string().contains("share id"));
    let mut s = tiny_sources();
    s.faces[2].r[0] = f32::NAN;
    assert!(compile(&s).is_err(), "NaN");
    let mut s = tiny_sources();
    s.grammar.clear();
    assert!(compile(&s).is_err(), "no grammar");
    let mut s = tiny_sources();
    s.dense.pop();
    assert!(compile(&s).is_err(), "list length");
    let mut s = tiny_sources();
    s.emotions.pop();
    assert!(compile(&s).is_err(), "18 emotions");
}

#[test]
fn compiled_tables_read_back() {
    for (name, e) in ENCODINGS {
        eprintln!("{name}");
        tables_read_back(tiny_as(e));
    }
}

/// The lossless encodings answer every query exactly as format 3.0 does.
#[test]
fn compact_is_lossless() {
    let a = Database::from_bytes(tiny_as(Encoding::legacy()), OpenOptions::default()).unwrap();
    let b = Database::from_bytes(tiny(), OpenOptions::default()).unwrap();
    let mut o = SearchOptions::legacy(20);
    o.explain = Explain::Full;
    for q in ["happy", "very sad", "not angry", "shrug", "sa", "(ツ)", "movie night", "so over it", "とても sad", "", "lewd", "bear", "hug"] {
        assert_eq!(a.search(q, &o), b.search(q, &o), "{q}");
        assert_eq!(a.search(q, &SearchOptions::default()), b.search(q, &SearchOptions::default()), "{q}");
    }
    let ea: Vec<Entry> = a.entries().collect();
    let eb: Vec<Entry> = b.entries().collect();
    assert_eq!(ea, eb, "every face's numbers, bit for bit");
    for e in &ea {
        assert_eq!(a.similar(e.id, &o), b.similar(e.id, &o));
    }
}

fn tables_read_back(bytes: Vec<u8>) {
    let db = Database::from_bytes(bytes, OpenOptions::default()).unwrap();
    let i = db.info();
    assert_eq!((i.faces, i.terms, i.phrases, i.situations, i.canonical, i.boosts), (14, 10, 2, 1, 2, 2));
    assert_eq!(db.manifest().languages, ["en", "ja"]);
    assert_eq!(db.manifest().phrase_max, 3);
    db.verify().unwrap();
    let shrug = db.find_text("¯\\_(ツ)_/¯").unwrap();
    assert_eq!(shrug.canonical_for, ["shrug"], "terms are lowercased; unknown texts dropped");
    let o = SearchOptions::legacy(10);
    assert_eq!(db.search("shrug", &o).hits[0].text, "¯\\_(ツ)_/¯");
    let r = db.search("happy", &o);
    assert_eq!(r.hits[0].text, "(^‿^)");
    assert_eq!(r.hits[1].text, "(◕‿◕✿)");
    // the japanese intensifier comes from the second GRAM section
    let mut o = SearchOptions::legacy(10);
    o.explain = Explain::Reading;
    assert!(db.search("とても sad", &o).reading.unwrap().line.starts_with("very sad"));
    assert_eq!(db.search("watching a film", &o).reading.unwrap().mode, ReadMode::Situation);
    assert!(db.search("so over it", &o).reading.unwrap().line.starts_with("over it"));
}

// ---- damaged files ----------------------------------------------------

/// Use every entry point on a database: none may panic.
fn exercise(db: &Database) {
    let mut o = SearchOptions::legacy(20);
    o.explain = Explain::Full;
    for q in ["happy", "very sad", "not angry", "shrug", "sa", "(ツ)", "movie night", "so over it", "とても sad", "", "lewd"] {
        let _ = db.search(q, &o);
        let _ = db.read(q, &o);
        let _ = db.search(q, &SearchOptions::default());
    }
    let _ = db.complete("s", 10);
    let _ = db.browse(&o);
    let first = db.entries().take(3).map(|e| e.id).collect::<Vec<_>>();
    for id in first {
        let _ = db.get(id);
        let _ = db.similar(id, &o);
    }
    let _ = db.find_text("(^‿^)");
    let _ = db.info();
    let _ = db.verify();
}

/// xorshift64*: a fixed sequence, so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn truncated_files_are_errors() {
    for (name, e) in ENCODINGS {
        eprintln!("{name}");
        truncated(tiny_as(e));
    }
}

fn truncated(bytes: Vec<u8>) {
    let dir_end = HEADER_LEN + 40 * DIR_ENTRY_LEN;
    for len in (0..bytes.len()).filter(|&l| l < dir_end || l % 7 == 0) {
        match Database::from_bytes(bytes[..len].to_vec(), OpenOptions::default()) {
            Ok(db) => {
                assert!(len + 8 > bytes.len(), "a file cut to {len} of {} bytes opened", bytes.len());
                exercise(&db);
            }
            Err(OpenError::Corrupt { .. } | OpenError::Incompatible { .. }) => {}
            Err(e) => panic!("unexpected error {e}"),
        }
    }
}

#[test]
fn flipped_bytes_never_panic() {
    for (name, e) in ENCODINGS {
        eprintln!("{name}");
        flipped(tiny_as(e));
    }
}

fn flipped(bytes: Vec<u8>) {
    let header_len = HEADER_LEN + bytes[24] as usize * DIR_ENTRY_LEN;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut opened, mut refused) = (0, 0);
    for round in 0..3000 {
        let mut b = bytes.clone();
        for _ in 0..1 + rng.below(4) {
            // half the flips in the header and directory, half anywhere
            let at = if round % 2 == 0 { rng.below(header_len) } else { rng.below(b.len()) };
            b[at] ^= 1 << rng.below(8);
            if rng.below(4) == 0 {
                b[at] = rng.next() as u8;
            }
        }
        match Database::from_bytes(b, OpenOptions::default()) {
            Ok(db) => {
                opened += 1;
                exercise(&db);
            }
            Err(_) => refused += 1,
        }
    }
    assert!(opened > 100 && refused > 100, "opened {opened}, refused {refused}");
}

/// The directory entry for a tag.
fn entry_at(b: &[u8], tag: &[u8; 4]) -> usize {
    let n = u32::from_le_bytes(b[24..28].try_into().unwrap()) as usize;
    (0..n).map(|k| HEADER_LEN + k * DIR_ENTRY_LEN).find(|&o| &b[o..o + 4] == tag).unwrap()
}

#[test]
fn directory_damage_gives_the_right_error() {
    let bytes = tiny_as(Encoding::legacy());
    let open = |b: Vec<u8>| Database::from_bytes(b, OpenOptions::default());

    // an unknown optional section is skipped; an unknown required one refuses the file
    let mut b = bytes.clone();
    let e = entry_at(&b, b"BOST");
    b[e..e + 4].copy_from_slice(b"ZZZZ");
    let db = open(b.clone()).unwrap();
    assert_eq!(db.info().boosts, 0);
    b[e + 4] |= SECTION_REQUIRED as u8;
    assert!(matches!(open(b), Err(OpenError::Incompatible { .. })));

    // a required section missing
    let mut b = bytes.clone();
    let e = entry_at(&b, b"FIDS");
    b[e..e + 4].copy_from_slice(b"ZZZZ");
    b[e + 4] &= !(SECTION_REQUIRED as u8);
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "FIDS" })));

    // wrong element size, misaligned offset, out of bounds
    let mut b = bytes.clone();
    let e = entry_at(&b, b"TERM");
    b[e + 24] ^= 4;
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "TERM" })));
    let mut b = bytes.clone();
    let e = entry_at(&b, b"FNUM");
    b[e + 8] ^= 4;
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "FNUM" })));
    let mut b = bytes.clone();
    let e = entry_at(&b, b"STRS");
    b[e + 16..e + 24].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "STRS" })));

    // lengths that disagree across sections
    let mut b = bytes.clone();
    let e = entry_at(&b, b"FTXT");
    let len = u64::from_le_bytes(b[e + 16..e + 24].try_into().unwrap());
    b[e + 16..e + 24].copy_from_slice(&(len - 8).to_le_bytes());
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "FTXT" })));

    // another format major
    let mut b = bytes.clone();
    b[8] = 4;
    assert!(matches!(open(b), Err(OpenError::Incompatible { .. })));

    // damage inside a section opens (no scan at open) but fails verify()
    let mut b = bytes.clone();
    let e = entry_at(&b, b"STRS");
    let off = u64::from_le_bytes(b[e + 8..e + 16].try_into().unwrap()) as usize;
    b[off + 3] ^= 0x40;
    let db = open(b).unwrap();
    assert_eq!(db.verify(), Err("STRS".to_string()));
    exercise(&db);
}

/// (offset, len) of a section.
fn section(b: &[u8], tag: &[u8; 4]) -> (usize, usize) {
    let e = entry_at(b, tag);
    let off = u64::from_le_bytes(b[e + 8..e + 16].try_into().unwrap()) as usize;
    let len = u64::from_le_bytes(b[e + 16..e + 24].try_into().unwrap()) as usize;
    (off, len)
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn compact_section_damage_never_panics() {
    let open = |b: Vec<u8>| Database::from_bytes(b, OpenOptions::default());
    let bytes = tiny();
    open(bytes.clone()).unwrap();

    // packed lists: a bad slot width or term count is refused at open
    for tag in [b"DNSP", b"ENGP"] {
        let name = std::str::from_utf8(tag).unwrap();
        let (off, _) = section(&bytes, tag);
        for bits in [0, 33, u32::MAX] {
            let mut b = bytes.clone();
            put_u32(&mut b, off, bits);
            assert!(matches!(open(b), Err(OpenError::Corrupt { section }) if section == name), "{name} bits {bits}");
        }
        let mut b = bytes.clone();
        put_u32(&mut b, off + 4, 9);
        assert!(matches!(open(b), Err(OpenError::Corrupt { .. })), "{name} n_terms");
        // starts out of order or out of range: opens, degrades, never panics
        for (j, v) in [(0, u32::MAX), (1, 0), (3, 1_000_000), (5, 2)] {
            let mut b = bytes.clone();
            put_u32(&mut b, off + 8 + 4 * j, v);
            exercise(&open(b).unwrap());
        }
    }

    // the code tables: offsets that decrease or run past the section
    let (off, len) = section(&bytes, b"FNCB");
    for (j, v) in [(1, u32::MAX), (3, 0), (27, u32::MAX), (27, len as u32)] {
        let mut b = bytes.clone();
        put_u32(&mut b, off + 4 * j, v);
        assert!(matches!(open(b), Err(OpenError::Corrupt { section: "FNCB" })), "FNCB[{j}] = {v}");
    }
    // codes beyond their table read as 0
    let (off, len) = section(&bytes, b"FNQ2");
    let mut b = bytes.clone();
    b[off..off + len].fill(0xff);
    let db = open(b).unwrap();
    exercise(&db);
    assert!(db.entries().all(|e| e.quality == 0.0 && e.attrs.suggestive == 0.0));

    // varint postings: garbage, endless varints, truncation
    let (off, len) = section(&bytes, b"PSTV");
    for fill in [0xff, 0x80, 0x00] {
        let mut b = bytes.clone();
        b[off..off + len].fill(fill);
        exercise(&open(b).unwrap());
    }

    // a missing code table, or char sets half there
    let mut b = bytes.clone();
    let e = entry_at(&b, b"FNCB");
    b[e..e + 4].copy_from_slice(b"ZZZZ");
    b[e + 4] &= !(SECTION_REQUIRED as u8);
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "FNCB" })));
    let legacy = tiny_as(Encoding::legacy());
    let mut b = legacy.clone();
    let e = entry_at(&b, b"CHRS");
    b[e..e + 4].copy_from_slice(b"ZZZZ");
    b[e + 4] &= !(SECTION_REQUIRED as u8);
    assert!(matches!(open(b), Err(OpenError::Corrupt { section: "CHRS" })));

    // random damage confined to the compact sections, which open() does not scan
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for small in [false, true] {
        let base = if small { tiny_as(Encoding::small()) } else { bytes.clone() };
        let tags: [&[u8; 4]; 5] = [if small { b"FNQ1" } else { b"FNQ2" }, b"FNCB", b"DNSP", b"ENGP", b"PSTV"];
        for round in 0..2000 {
            let mut b = base.clone();
            let (off, len) = section(&b, tags[round % tags.len()]);
            for _ in 0..1 + rng.below(6) {
                let at = off + rng.below(len.max(1));
                b[at] = rng.next() as u8;
            }
            if let Ok(db) = open(b) {
                exercise(&db);
            }
        }
    }
}

/// A 3.0 reader refuses a compact file as Incompatible: its sections are
/// flagged required. (Simulated: an unknown required tag.)
#[test]
fn compact_sections_are_required() {
    let b = tiny();
    for tag in [b"FNQ2", b"FNCB", b"DNSP", b"ENGP", b"PSTV"] {
        let e = entry_at(&b, tag);
        assert_eq!(b[e + 4] & SECTION_REQUIRED as u8, SECTION_REQUIRED as u8, "{tag:?}");
    }
}
