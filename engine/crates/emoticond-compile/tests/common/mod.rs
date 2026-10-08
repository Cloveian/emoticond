//! Shared test sources: the tiny fixture's faces, terms and tables.
#![allow(dead_code)]

use emoticond::data::format::{NONE, N_EMO};
use emoticond::*;
use emoticond_compile::*;

pub const EMOTIONS: [&str; N_EMO] = [
    "happy", "laughing", "excited", "love", "flirty", "playful", "smug", "proud", "calm", "sleepy", "sad", "angry",
    "scared", "surprised", "confused", "shy", "sorry", "bored", "disgusted",
];

pub fn prof(pairs: &[(&str, f32)]) -> [f32; N_EMO] {
    let mut p = [0f32; N_EMO];
    for (e, v) in pairs {
        p[EMOTIONS.iter().position(|x| x == e).unwrap()] = *v;
    }
    let s: f32 = p.iter().sum();
    if s > 0.0 {
        p.iter_mut().for_each(|x| *x /= s);
    }
    p
}

pub fn face(text: &str, r: &[(&str, f32)], words: &[&str]) -> FaceIn {
    let mut rr = [0f32; N_EMO];
    for (e, v) in r {
        rr[EMOTIONS.iter().position(|x| x == e).unwrap()] = *v;
    }
    FaceIn {
        text: text.into(),
        r: rr,
        face: 1.0,
        quality: 5.5,
        words: words.iter().enumerate().map(|(i, w)| (w.to_string(), (i.min(2)) as u8)).collect(),
        ..FaceIn::default()
    }
}

pub fn term(key: &str, p: &[(&str, f32)], n: u32, tier: u32) -> TermIn {
    TermIn { key: key.into(), p: prof(p), m: Some(0.2), n, tier, weak: false }
}

/// A dozen faces, a handful of terms, one of each other table.
pub fn tiny_sources() -> Sources {
    let mut faces = vec![
        face("¯\\_(ツ)_/¯", &[("confused", 0.8), ("smug", 0.3)], &["shrug", "arms"]),
        face("(╯°□°)╯︵ ┻━┻", &[("angry", 0.9), ("surprised", 0.2)], &["table flip", "angry"]),
        face("(^‿^)", &[("happy", 0.8), ("calm", 0.3)], &["smile", "happy"]),
        face("(╥﹏╥)", &[("sad", 0.9)], &["crying", "sad"]),
        face("(づ｡◕‿‿◕｡)づ", &[("love", 0.7), ("happy", 0.5)], &["hug", "arms"]),
        face("(*/ω＼*)", &[("shy", 0.9), ("happy", 0.2)], &["shy", "hiding"]),
        face("( ͡° ͜ʖ ͡°)", &[("smug", 0.8), ("flirty", 0.4)], &["lenny"]),
        face("(◕‿◕✿)", &[("happy", 0.9), ("playful", 0.3)], &["flower", "cute", "happy"]),
        face("┌∩┐(◣_◢)┌∩┐", &[("angry", 1.0)], &["middle finger", "angry"]),
        face("(˘ε˘)", &[("flirty", 0.8), ("love", 0.5)], &["kiss"]),
        face("ʕ•ᴥ•ʔ", &[("calm", 0.7), ("happy", 0.4)], &["bear", "cute"]),
        face("(⊙_⊙)", &[("surprised", 0.9), ("scared", 0.3)], &["stare", "surprised"]),
        face("(￣ω￣;)", &[("sorry", 0.6), ("shy", 0.3)], &["sweat"]),
        face("(っ˘ω˘ς )", &[("sleepy", 0.9)], &["sleepy"]),
    ];
    faces[4].multi = 0.9;
    faces[6].lenny = 1.0;
    faces[6].suggestive = 1.0;
    faces[7].cute = 0.9;
    faces[8].crude = 1.0;
    faces[9].suggestive = 1.8;
    faces[10].cute = 0.8;
    faces[13].quality = 3.0;
    let vocab = vec![
        term("happy", &[("happy", 1.0)], 9, 2),
        term("sad", &[("sad", 1.0)], 5, 2),
        term("angry", &[("angry", 1.0)], 6, 2),
        term("shrug", &[("confused", 0.7), ("smug", 0.3)], 3, 1),
        term("hug", &[("love", 0.6), ("happy", 0.4)], 4, 1),
        term("shy", &[("shy", 1.0)], 3, 2),
        term("surprised", &[("surprised", 1.0)], 3, 2),
        term("table flip", &[("angry", 0.8), ("surprised", 0.2)], 2, 1),
        term("sleepy", &[("sleepy", 1.0)], 2, 2),
        TermIn { key: "bear".into(), p: prof(&[("calm", 1.0)]), m: None, n: 1, tier: 0, weak: true },
    ];
    let k = 4;
    let lists: [[u32; 4]; 10] = [
        [2, 7, 10, 4],
        [3, 12, NONE, NONE],
        [1, 8, 11, NONE],
        [0, 12, 11, NONE],
        [4, 9, 2, NONE],
        [5, 12, 2, NONE],
        [11, 1, NONE, NONE],
        [1, 8, NONE, NONE],
        [13, NONE, NONE, NONE],
        [10, 99, NONE, NONE], // 99: a row the export does not have
    ];
    let dense: Vec<u32> = lists.iter().flatten().copied().collect();
    let mut grammar = Grammar::new("en");
    grammar.intensifiers = vec!["very".into(), "really".into(), "so".into()];
    grammar.diminishers = vec!["kinda".into(), "bit".into()];
    grammar.two_word_diminishers = vec!["kind of".into(), "a bit".into()];
    grammar.negators = vec!["not".into()];
    grammar.stopwords = vec!["i".into(), "am".into(), "feel".into(), "a".into(), "the".into(), "face".into()];
    grammar.lewd = vec!["lewd".into(), "nsfw".into()];
    grammar.single_words = vec!["face".into(), "solo".into()];
    grammar.pair_words = vec!["together".into(), "pair".into()];
    grammar.emoticons = vec![":( sad".into(), "xD laughing".into()];
    let mut ja = Grammar::new("ja");
    ja.intensifiers = vec!["とても".into()];
    Sources {
        params: Params { data_version: "test".into(), ..Params::default() },
        emotions: EMOTIONS.iter().map(|s| s.to_string()).collect(),
        extras: ["multi", "cute", "intensity", "suggestive", "lenny", "face"].iter().map(|s| s.to_string()).collect(),
        faces,
        vocab,
        dense_k: k,
        engine: Some(dense.iter().rev().copied().collect()),
        dense,
        phrases: vec![
            PhraseIn {
                key: "over it".into(),
                p: prof(&[("bored", 0.6), ("angry", 0.4)]),
                level: Some(2),
                spellings: vec!["so over it".into(), "done with this".into()],
                words: vec!["shrug".into(), "sweat".into()],
                ..PhraseIn::default()
            },
            PhraseIn {
                key: "uwu".into(),
                p: prof(&[("happy", 0.5), ("shy", 0.5)]),
                level: Some(1),
                cute: true,
                ..PhraseIn::default()
            },
        ],
        situations: vec![SituationIn {
            name: "movie night".into(),
            matches: vec!["movie night".into(), "watching a film".into()],
            p: prof(&[("excited", 0.5), ("happy", 0.5)]),
            pair: Some(true),
            words: vec!["hug".into()],
        }],
        canonical: vec![
            CanonIn { term: "happy".into(), text: "(^‿^)".into(), rank: 1, hand: false },
            CanonIn { term: "happy".into(), text: "(◕‿◕✿)".into(), rank: 2, hand: false },
            CanonIn { term: "Shrug".into(), text: "¯\\_(ツ)_/¯".into(), rank: 1, hand: true },
            CanonIn { term: "shrug".into(), text: "not in the data".into(), rank: 1, hand: true },
        ],
        boosts: vec![
            BoostIn { term: "happy".into(), id: FaceId::of_text("ʕ•ᴥ•ʔ"), boost: 2.0 },
            BoostIn { term: "happy".into(), id: FaceId::of_text("ʕ•ᴥ•ʔ"), boost: 3.0 },
            BoostIn { term: "sad".into(), id: FaceId::of_text("(￣ω￣;)"), boost: -1.0 },
        ],
        grammar: vec![ja, grammar],
        licence: "CC BY 4.0 (test fixture)\n".into(),
        attribution: "hand-written test faces\n".into(),
        source_rev: "tiny".into(),
        aliases: Vec::new(),
    }
}

