//! Data sets, encodings, aliases and the public policy, on sources written
//! in code (no real data needed).

use emoticond::data::format::{N_EMO, NONE, RETIRED_LICENCE};
use emoticond::*;
use emoticond_compile::legacy::{parse_aliases, LegacyPaths};
use emoticond_compile::*;
use std::collections::BTreeSet;
use std::path::Path;

mod common;
use common::{tiny_sources, EMOTIONS};

fn open(bytes: Vec<u8>) -> Database {
    Database::from_bytes(bytes, OpenOptions::default()).unwrap()
}

fn with(e: Encoding, f: impl FnOnce(&mut Sources)) -> Vec<u8> {
    let mut s = tiny_sources();
    s.params.encoding = e;
    f(&mut s);
    compile(&s).unwrap()
}

// ---- sets ----------------------------------------------------------------

#[test]
fn a_set_keeps_its_faces_and_every_rank() {
    let keep = ["(^‿^)", "(◕‿◕✿)", "ʕ•ᴥ•ʔ", "(╥﹏╥)", "¯\\_(ツ)_/¯"];
    let mut s = tiny_sources();
    let before = s.dense.clone();
    let texts: Vec<String> = s.faces.iter().map(|f| f.text.clone()).collect();
    assert_eq!(s.retain_faces(|f| keep.contains(&f.text.as_str())), keep.len());
    // a slot keeps its rank: it names the same face, or none
    for (a, b) in before.iter().zip(&s.dense) {
        match texts.get(*a as usize) {
            Some(t) if keep.contains(&t.as_str()) => assert_eq!(s.faces[*b as usize].text, *t),
            _ => assert_eq!(*b, NONE),
        }
    }
    s.truncate_neighbours(2);
    assert_eq!((s.dense_k, s.dense.len(), s.engine.as_ref().unwrap().len()), (2, 20, 20));
    for e in [Encoding::legacy(), Encoding::compact(), Encoding::small()] {
        let mut s2 = s.clone();
        s2.params.encoding = e;
        let a = compile(&s2).unwrap();
        assert_eq!(a, compile(&s2).unwrap(), "deterministic");
        let db = open(a);
        assert_eq!(db.info().faces, keep.len());
        assert_eq!(db.manifest().dense_k, 2);
        assert_eq!(db.search("happy", &SearchOptions::legacy(3)).hits[0].text, "(^‿^)");
        // canonical picks of dropped faces fall away; boosts are kept by id
        assert!(db.find_text("(╯°□°)╯︵ ┻━┻").is_none());
    }
    // legacy and compact sets answer alike
    let mut l = s.clone();
    l.params.encoding = Encoding::legacy();
    let (a, b) = (open(compile(&l).unwrap()), open(compile(&s).unwrap()));
    for q in ["happy", "sad", "shrug", "bear", "very happy", "sa"] {
        assert_eq!(a.search(q, &SearchOptions::legacy(10)), b.search(q, &SearchOptions::legacy(10)), "{q}");
    }
}

// ---- quantised face numbers -------------------------------------------------

/// Sources with many distinct values per field (so q8 must quantise),
/// including values on and around every threshold the engine tests.
fn noisy_sources() -> Sources {
    let mut s = tiny_sources();
    let mut x = 0x9E37_79B9u64;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % 100_000) as f32 / 100_000.0
    };
    let edges = [0.5f32, 1.5, 0.4999, 0.5001, 1.4999, 1.5001];
    for i in 0..1200 {
        let mut f = common::face(&format!("(f{i})"), &[], &["noise"]);
        for r in f.r.iter_mut() {
            *r = rnd();
        }
        f.multi = if i % 50 == 0 { 0.5 } else { rnd() };
        f.cute = rnd();
        f.intensity = rnd();
        f.suggestive = if i % 7 == 0 { edges[i % edges.len()] } else { 3.0 * rnd() };
        f.lenny = if i % 11 == 0 { 0.5 } else { rnd() };
        f.face = if i % 13 == 0 { 0.5 } else { rnd() };
        f.quality = 1.0 + 7.0 * rnd();
        f.crude = if i % 17 == 0 { 0.5 } else { rnd() };
        s.faces.push(f);
    }
    s
}

#[test]
fn q16_is_exact_and_q8_is_within_a_step() {
    let s = noisy_sources();
    let exact = |e: Encoding| {
        let mut s = s.clone();
        s.params.encoding = e;
        open(compile(&s).unwrap())
    };
    let (f32s, q16, q8) = (exact(Encoding::legacy()), exact(Encoding::compact()), exact(Encoding::small()));
    let a: Vec<Entry> = f32s.entries().collect();
    assert_eq!(a, q16.entries().collect::<Vec<_>>(), "q16 is bit for bit");
    let b: Vec<Entry> = q8.entries().collect();
    // a step: the field's range over at most 255 bins
    let step = |lo: f32, hi: f32| (hi - lo) / 249.0;
    let mut worst = 0f32;
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.id, y.id);
        let pairs = [
            (x.attrs.multi, y.attrs.multi, 1.0),
            (x.attrs.cute, y.attrs.cute, 1.0),
            (x.attrs.intensity, y.attrs.intensity, 1.0),
            (x.attrs.suggestive, y.attrs.suggestive, 3.0),
            (x.attrs.lenny, y.attrs.lenny, 1.0),
            (x.attrs.face, y.attrs.face, 1.0),
            (x.quality, y.quality, 7.0),
        ];
        for (u, v, range) in pairs {
            assert!((u - v).abs() <= step(0.0, range), "{u} vs {v}");
            worst = worst.max((u - v).abs() / range);
        }
        for ((_, u), (_, v)) in x.emotions.iter().zip(&y.emotions) {
            assert!((u - v).abs() <= step(0.0, 1.0), "{u} vs {v}");
        }
        // every threshold keeps its side: the flags agree
        assert_eq!(x.flags, y.flags, "{}: {:?} vs {:?}", x.text, x.attrs, y.attrs);
    }
    assert!(worst > 0.0, "q8 did quantise");
    assert_eq!(q8.manifest().get("encoding").map(|e| e.starts_with("nums=q8")), Some(true));
}

// ---- aliases ----------------------------------------------------------------

fn aliased() -> (Database, FaceId, FaceId, FaceId, FaceId) {
    let shrug = FaceId::of_text("¯\\_(ツ)_/¯");
    let (old, older, gone) = (FaceId::from_u64(0x1111_2222_3333).unwrap(), FaceId::from_u64(0x0000_0000_0001).unwrap(), FaceId::from_u64(0x7777_0000_0000).unwrap());
    let text = format!(
        "# renamed twice, then retired\n{{\"old\": \"{older}\", \"new\": \"{old}\"}}\n{{\"old\": \"{old}\", \"new\": \"{shrug}\", \"why\": \"stripped\"}}\n\n{{\"old\": \"{gone}\", \"retired\": \"licence\"}}\n"
    );
    let aliases = parse_aliases(&text).unwrap();
    let db = open(with(Encoding::compact(), |s| s.aliases = aliases));
    (db, shrug, old, older, gone)
}

#[test]
fn aliases_resolve_everywhere_ids_come_in() {
    let (db, shrug, old, older, gone) = aliased();
    assert_eq!(db.manifest().get("n_aliases"), Some("2"));
    assert_eq!(db.id_status(shrug), IdStatus::Live);
    assert_eq!(db.id_status(old), IdStatus::Aliased(shrug));
    assert_eq!(db.id_status(older), IdStatus::Aliased(shrug), "chains resolve to their end");
    assert_eq!(db.id_status(gone), IdStatus::Retired(RETIRED_LICENCE));
    assert_eq!(db.id_status(FaceId::from_u64(42).unwrap()), IdStatus::Unknown);
    // get
    assert_eq!(db.get(old).unwrap().id, shrug);
    assert_eq!(db.get(older).unwrap().text, "¯\\_(ツ)_/¯");
    assert!(db.get(gone).is_none());
    // exclude
    let mut o = SearchOptions::legacy(10);
    assert_eq!(db.search("shrug", &o).hits[0].id, shrug);
    o.exclude.insert(older);
    assert!(db.search("shrug", &o).hits.iter().all(|h| h.id != shrug));
    // usage: the old id's picks count for the face, and add to the new id's
    let base = db.search("arms", &SearchOptions::legacy(10));
    let mut u = SearchOptions::legacy(10);
    u.usage_weight = UsageWeight::High;
    u.usage.global.insert(old, 1.0);
    let lifted = db.search("arms", &u);
    let score = |r: &SearchResult| r.hits.iter().find(|h| h.id == shrug).map(|h| h.score).unwrap();
    assert!(score(&lifted) > score(&base), "{} -> {}", score(&base), score(&lifted));
    let mut n = SearchOptions::legacy(10);
    n.usage_weight = UsageWeight::High;
    n.usage.global.insert(shrug, 1.0);
    assert_eq!(db.search("arms", &n), lifted, "the old id counts as the new one");
    // the same weight under the old and the new id together is deterministic
    u.usage.global.insert(shrug, 0.5);
    assert_eq!(db.search("arms", &u), db.search("arms", &u));
    // blocklist
    db.set_blocklist(BTreeSet::from([old]));
    assert!(db.search("shrug", &SearchOptions::legacy(10)).hits.iter().all(|h| h.id != shrug));
}

#[test]
fn bad_aliases_are_refused() {
    let shrug = FaceId::of_text("¯\\_(ツ)_/¯");
    let a = FaceId::from_u64(0xaaaa).unwrap();
    let b = FaceId::from_u64(0xbbbb).unwrap();
    let bad = |aliases: Vec<AliasIn>| {
        let mut s = tiny_sources();
        s.aliases = aliases;
        compile(&s).unwrap_err().to_string()
    };
    assert!(bad(vec![AliasIn { old: shrug, new: Some(a), reason: 0 }]).contains("is a face of this set"));
    assert!(bad(vec![AliasIn { old: a, new: Some(b), reason: 0 }, AliasIn { old: b, new: Some(a), reason: 0 }]).contains("cycle"));
    assert!(bad(vec![AliasIn { old: a, new: Some(shrug), reason: 0 }, AliasIn { old: a, new: None, reason: 1 }]).contains("both"));
    for line in ["{\"old\": \"kzz\", \"new\": \"k000000000001\"}", "{\"new\": \"k000000000001\"}", "{\"old\": \"k000000000001\"}", "{\"old\": \"k000000000001\", \"retired\": \"bored\"}", "not json"] {
        assert!(parse_aliases(line).is_err(), "{line}");
    }
    // without aliases the file stays format 3.0 under the legacy encoding
    let b = with(Encoding::legacy(), |_| {});
    assert_eq!(&b[10..12], &[0, 0]);
    let b = with(Encoding::legacy(), |s| s.aliases = vec![AliasIn { old: a, new: Some(shrug), reason: 0 }]);
    assert_eq!(&b[10..12], &[1, 0], "ALIA makes it 3.1");
    let db = open(b);
    assert_eq!(db.get(a).unwrap().id, shrug);
}

// ---- the public policy (the legacy adapter) ---------------------------------

/// A minimal export and data dir in a temp dir.
fn export_dir(name: &str, meta_policy: Option<&str>, wc: &[&str], ev: &[&str]) -> LegacyPaths {
    let root = std::env::temp_dir().join(format!("emoticond-test-policy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let eng = root.join("engine");
    let data = root.join("data");
    std::fs::create_dir_all(eng.clone()).unwrap();
    std::fs::create_dir_all(data.join("grammar")).unwrap();
    std::fs::create_dir_all(data.join("licence")).unwrap();
    let mut meta = serde_json::json!({"emotions": EMOTIONS, "extra": ["multi", "cute", "intensity", "suggestive", "lenny", "face"], "dense_k": 2});
    if let Some(p) = meta_policy {
        meta["policy"] = p.into();
        meta["tag_sources"] = serde_json::json!(["kmoji"]);
    }
    let w = |p: &Path, s: &str| std::fs::write(p, s).unwrap();
    w(&eng.join("meta.json"), &meta.to_string());
    let words: Vec<serde_json::Value> = wc.iter().enumerate().map(|(i, _)| serde_json::json!([format!("w{i}"), 0])).collect();
    let mut face = serde_json::json!({"id": 1, "text": "(^‿^)", "r": vec![0.1; N_EMO], "x": [0, 0, 0, 0, 0, 1], "q": 5.0, "w": words});
    if !wc.is_empty() {
        face["wc"] = serde_json::json!(wc);
    }
    let face2 = serde_json::json!({"id": 2, "text": "(╥﹏╥)", "r": vec![0.2; N_EMO], "x": [0, 0, 0, 0, 0, 1], "q": 5.0, "w": []});
    w(&eng.join("faces.jsonl"), &format!("{face}\n{face2}\n"));
    let mut v = serde_json::json!({"t": "happy", "p": {"happy": 1.0}, "n": 1, "src": "cat"});
    if meta_policy.is_some() {
        v["ev"] = serde_json::json!(ev);
    }
    w(&eng.join("vocab.jsonl"), &format!("{v}\n"));
    std::fs::write(eng.join("dense.bin"), [1u32, 2].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    for f in ["lexicon_phrases.jsonl", "boosts.jsonl"] {
        w(&data.join(f), "");
    }
    w(&data.join("grammar/en.json"), "{\"lang\": \"en\", \"stopwords\": [\"a\"]}");
    w(&data.join("licence/LICENCE.txt"), "CC BY 4.0\n");
    w(&data.join("licence/ATTRIBUTION.txt"), "test\n");
    let situations = root.join("situations.json");
    w(&situations, "{}");
    LegacyPaths { engine_dir: eng, data_dir: data.clone(), situations, aliases: data.join("aliases.jsonl") }
}

fn public() -> Params {
    Params { policy: emoticond_compile::policy::public_policy_id(), ..Params::default() }
}

#[test]
fn a_public_build_refuses_non_shippable_classes() {
    // clean: compiles, flagged public
    let ok = export_dir("ok", Some("public"), &["a,m", "s:kmoji"], &["own:cat", "s:emojicombos"]);
    let bytes = compile(&ok.read(public()).unwrap()).unwrap();
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()) & 1, 1, "header flag bit 0");
    assert!(open(bytes).manifest().policy.starts_with("public@"));
    let refused = |p: &LegacyPaths| match p.read(public()) {
        Err(CompileError::Policy(m)) => m,
        other => panic!("not refused: {:?}", other.map(|_| ())),
    };
    // a face word of a limited source
    let m = refused(&export_dir("ekohrt", Some("public"), &["a", "s:ekohrt,a"], &["own:cat"]));
    assert!(m.contains("face words of class s:ekohrt: 1"), "{m}");
    let m = refused(&export_dir("multi", Some("public"), &["s:emojicombos/multi"], &["own:cat"]));
    assert!(m.contains("s:emojicombos/multi"), "{m}");
    // vocab evidence that does not count
    let m = refused(&export_dir("list", Some("public"), &["a"], &["list:emojidb"]));
    assert!(m.contains("vocab evidence list:emojidb: 1"), "{m}");
    // missing labels, or an export that is not public at all
    let m = refused(&export_dir("unlabelled", Some("public"), &[], &[]));
    assert!(m.contains("without evidence"), "{m}");
    let m = refused(&export_dir("private", None, &["a"], &["own:cat"]));
    assert!(m.contains("meta.json policy"), "{m}");
    // a private build of any of them is fine
    let p = export_dir("private-ok", None, &["a"], &[]);
    assert!(p.read(Params::default()).is_ok());
    let p = export_dir("ekohrt-private", Some("public"), &["s:ekohrt"], &["list:emojidb"]);
    assert!(p.read(Params::default()).is_ok());
}

