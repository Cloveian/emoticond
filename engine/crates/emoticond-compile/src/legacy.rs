//! The adapter from today's private inputs to [`Sources`]:
//!
//! | input | path (in a checkout) |
//! |---|---|
//! | the engine export (`pipeline/export_engine2.py`) | `work/engine/{meta.json, faces.jsonl, vocab.jsonl, dense.bin, engine.bin}` |
//! | canonical picks | `data/canonical_hand.jsonl`, `data/canonical.jsonl` |
//! | the generated search lexicon | `data/lexicon_phrases.jsonl` |
//! | curated boosts | `data/boosts.jsonl` |
//! | situations | `data/situations.json` |
//! | grammar word lists | `data/grammar/<lang>.json` |
//! | licence and attribution | `data/licence/LICENCE.txt`, `data/licence/ATTRIBUTION.txt` |
//!
//! Parsing follows the pre-v3 engine exactly (the same defaults, the same
//! float arithmetic, the same skipping of unreadable lines), so a data file
//! compiled from these inputs ranks exactly as the old index did.
//!
//! The source bundle will be
//! a second adapter onto [`Sources`].

use crate::policy::Violations;
use crate::{AliasIn, BoostIn, CanonIn, CompileError, FaceIn, Params, PhraseIn, SituationIn, Sources, TermIn};
use emoticond::data::format::{N_EMO, NONE, RETIRED_BLOCKLISTED, RETIRED_LICENCE, RETIRED_OTHER, RETIRED_PRUNED, RETIRED_QUALITY};
use emoticond::{FaceId, Grammar};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The export's files, in `engine_dir`.
pub const EXPORT_FILES: [&str; 5] = ["meta.json", "faces.jsonl", "vocab.jsonl", "dense.bin", "engine.bin"];

/// Where each input lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyPaths {
    /// The engine export (`work/engine/`).
    pub engine_dir: PathBuf,
    /// The curated data (`data/`): canonical, phrases, boosts, grammar/, licence/.
    pub data_dir: PathBuf,
    /// `data/situations.json`.
    pub situations: PathBuf,
    /// Renamed and retired ids (`data/aliases.jsonl`; optional).
    pub aliases: PathBuf,
}

impl LegacyPaths {
    /// The inputs in a checkout of the private repo, with the export in
    /// `engine_dir` (normally `<repo>/work/engine`).
    pub fn for_repo(repo: &Path, engine_dir: &Path) -> LegacyPaths {
        LegacyPaths {
            engine_dir: engine_dir.to_path_buf(),
            data_dir: repo.join("data"),
            situations: repo.join("data/situations.json"),
            aliases: repo.join("data/aliases.jsonl"),
        }
    }

    fn data(&self, name: &str) -> PathBuf {
        self.data_dir.join(name)
    }

    fn grammar_dir(&self) -> PathBuf {
        self.data_dir.join("grammar")
    }

    fn grammar_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(self.grammar_dir())
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Every input file (for staleness checks). The export's `engine.bin`
    /// and the grammar files are included when present.
    pub fn inputs(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = EXPORT_FILES.iter().map(|f| self.engine_dir.join(f)).collect();
        for f in ["canonical_hand.jsonl", "canonical.jsonl", "lexicon_phrases.jsonl", "boosts.jsonl", "licence/LICENCE.txt", "licence/ATTRIBUTION.txt"] {
            v.push(self.data(f));
        }
        v.push(self.situations.clone());
        v.push(self.aliases.clone());
        v.extend(self.grammar_files());
        v
    }

    /// Whether the export is here at all (`faces.jsonl` and friends).
    pub fn has_export(&self) -> bool {
        ["meta.json", "faces.jsonl", "vocab.jsonl", "dense.bin"].iter().all(|f| self.engine_dir.join(f).is_file())
    }

    /// Whether `out` must be (re)built: missing, or older than any input
    /// that exists (make's rule). Missing inputs are the compiler's error.
    pub fn is_stale(&self, out: &Path) -> bool {
        let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let Some(built) = mtime(out) else { return true };
        // the grammar dir itself, so a deleted or added language counts
        let dir = std::iter::once(self.grammar_dir());
        self.inputs().iter().cloned().chain(dir).filter_map(|p| mtime(&p)).any(|t: SystemTime| t > built)
    }

    /// Read every input into [`Sources`].
    pub fn read(&self, params: Params) -> Result<Sources, CompileError> {
        read(self, params)
    }
}

fn io_err(path: &Path, e: std::io::Error) -> CompileError {
    CompileError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

fn read_file(path: &Path) -> Result<Vec<u8>, CompileError> {
    std::fs::read(path).map_err(|e| io_err(path, e))
}

fn read_text(path: &Path) -> Result<String, CompileError> {
    String::from_utf8(read_file(path)?).map_err(|_| CompileError::Invalid(format!("{} is not UTF-8", path.display())))
}

/// An emotion profile from a JSON object of name -> weight, normalised to
/// sum 1 (exactly the pre-v3 engine's arithmetic).
pub fn profile(emotions: &[String], obj: Option<&serde_json::Value>) -> [f32; N_EMO] {
    let mut p = [0f32; N_EMO];
    if let Some(o) = obj.and_then(|v| v.as_object()) {
        for (k, v) in o {
            if let Some(i) = emotions.iter().position(|e| e == k) {
                p[i] = v.as_f64().unwrap_or(0.0) as f32;
            }
        }
    }
    let s: f32 = p.iter().sum();
    if s > 0.0 {
        for x in p.iter_mut() {
            *x /= s;
        }
    }
    p
}

#[derive(Deserialize)]
struct FaceLine {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    r: Vec<f64>,
    #[serde(default)]
    x: Vec<f64>,
    q: Option<f64>,
    #[serde(default)]
    w: Vec<(String, u64)>,
    #[serde(default)]
    c: i64,
    /// provenance classes per word (public exports)
    wc: Option<Vec<String>>,
}

fn strs(v: &serde_json::Value, n: &str) -> Vec<String> {
    v[n].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn read(paths: &LegacyPaths, mut params: Params) -> Result<Sources, CompileError> {
    let bad = |m: String| CompileError::Invalid(m);
    let dir = &paths.engine_dir;
    let mut digest: Vec<(String, Vec<u8>)> = Vec::new();

    // meta
    let meta_raw = read_file(&dir.join("meta.json"))?;
    let meta: serde_json::Value = serde_json::from_slice(&meta_raw).map_err(|e| bad(format!("meta.json: {e}")))?;
    let emotions: Vec<String> = strs(&meta, "emotions");
    if emotions.len() != N_EMO {
        return Err(bad("meta.json does not list the 19 emotions".into()));
    }
    let extras = strs(&meta, "extra");
    let dense_k = meta["dense_k"].as_u64().unwrap_or(100) as usize;
    // a public build: the export must say so, and every label must be allowed
    let public = params.policy.starts_with("public");
    let mut violations = Violations::default();
    if public {
        let p = meta["policy"].as_str().unwrap_or("");
        if p != "public" {
            violations.meta_policy = Some(if p.is_empty() { "(none)".into() } else { p.into() });
        }
        for s in meta["tag_sources"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
            if !crate::policy::face_class_ok(&format!("s:{s}")) {
                *violations.face.entry(format!("s:{s} (meta.json tag_sources)")).or_default() += 1;
            }
        }
    }
    if let Some(q) = meta["quality_cut"].as_f64() {
        params.quality_cut = Some(q as f32);
    }
    digest.push(("meta.json".into(), meta_raw));

    // faces, one line at a time (the pre-v3 compile's parsing)
    let mut faces = Vec::new();
    let mut by_row: HashMap<u32, u32> = HashMap::new();
    let path = dir.join("faces.jsonl");
    let mut hasher = crate::Sha256Stream::default();
    let reader = std::io::BufReader::new(std::fs::File::open(&path).map_err(|e| io_err(&path, e))?);
    for line in reader.lines() {
        let line = line.map_err(|e| io_err(&path, e))?;
        hasher.update(line.as_bytes());
        let Ok(v) = serde_json::from_str::<FaceLine>(&line) else { continue };
        let at = |a: &[f64], i: usize| a.get(i).copied().unwrap_or(0.0) as f32;
        if public {
            violations.check_face(v.w.len(), v.wc.as_deref());
        }
        by_row.insert(v.id as u32, faces.len() as u32);
        let mut r = [0f32; N_EMO];
        for (i, x) in r.iter_mut().enumerate() {
            *x = at(&v.r, i);
        }
        faces.push(FaceIn {
            r,
            multi: at(&v.x, 0) / 3.0,
            cute: at(&v.x, 1) / 3.0,
            intensity: at(&v.x, 2) / 3.0,
            suggestive: at(&v.x, 3),
            lenny: at(&v.x, 4),
            face: at(&v.x, 5),
            quality: v.q.unwrap_or(5.0) as f32,
            crude: if v.c != 0 { 1.0 } else { 0.0 },
            words: v.w.into_iter().map(|(w, t)| (w, t.min(2) as u8)).collect(),
            text: v.text,
        });
    }
    digest.push(("faces.jsonl".into(), hasher.finish()));

    // vocab: every line, in order
    let vocab_raw = read_text(&dir.join("vocab.jsonl"))?;
    let mut vocab = Vec::new();
    for line in vocab_raw.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let src = v["src"].as_str().unwrap_or("");
        if public {
            let ev = v.get("ev").and_then(|e| e.as_array()).map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect::<Vec<_>>());
            violations.check_key(ev.as_deref());
        }
        vocab.push(TermIn {
            key: v["t"].as_str().unwrap_or("").to_string(),
            p: profile(&emotions, v.get("p")),
            m: v["m"].as_f64().map(|x| x as f32),
            n: v["n"].as_u64().unwrap_or(0) as u32,
            tier: if src == "cat" {
                2
            } else if src.contains("engine") {
                1
            } else {
                0
            },
            weak: src == "e5",
        });
    }
    digest.push(("vocab.jsonl".into(), vocab_raw.into_bytes()));
    if !violations.is_empty() {
        return Err(CompileError::Policy(violations.report()));
    }

    // neighbour lists: export row ids -> face indices
    // as_chunks would need Rust 1.88; the MSRV is 1.85
    #[allow(clippy::chunks_exact_to_as_chunks, unknown_lints)]
    let lists = |raw: &[u8]| -> Vec<u32> {
        raw.chunks_exact(4)
            .map(|b| *by_row.get(&u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(&NONE))
            .collect()
    };
    let dense_raw = read_file(&dir.join("dense.bin"))?;
    let dense = lists(&dense_raw);
    if dense.len() != vocab.len() * dense_k {
        return Err(bad("dense.bin does not match vocab.jsonl".into()));
    }
    digest.push(("dense.bin".into(), dense_raw));
    let engine = match std::fs::read(dir.join("engine.bin")) {
        Ok(raw) => {
            let l = lists(&raw);
            digest.push(("engine.bin".into(), raw));
            (l.len() == vocab.len() * dense_k).then_some(l)
        }
        Err(_) => None,
    };

    // canonical picks: hand-curated first, then the agent's
    let mut canonical = Vec::new();
    for name in ["canonical_hand.jsonl", "canonical.jsonl"] {
        let p = paths.data(name);
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io_err(&p, e)),
        };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let (Some(term), Some(t)) = (v["term"].as_str(), v["text"].as_str()) else { continue };
            canonical.push(CanonIn {
                term: term.to_string(),
                text: t.to_string(),
                rank: v["rank"].as_u64().unwrap_or(1),
                hand: v["src"].as_str() == Some("hand"),
            });
        }
        digest.push((name.into(), text.into_bytes()));
    }

    // phrases
    let text = read_text(&paths.data("lexicon_phrases.jsonl"))?;
    let mut phrases = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(key) = v["key"].as_str() else { continue };
        let attr = strs(&v, "attr");
        phrases.push(PhraseIn {
            key: key.to_string(),
            p: profile(&emotions, v.get("p")),
            level: v["level"].as_u64().map(|x| x as u8),
            pair: v["pair"].as_bool(),
            spellings: strs(&v, "match"),
            words: strs(&v, "words"),
            cute: attr.iter().any(|a| a == "cute"),
            lenny: attr.iter().any(|a| a == "lenny"),
            lewd: attr.iter().any(|a| a == "suggestive"),
            over: v["over"].as_bool().unwrap_or(false),
        });
    }
    digest.push(("lexicon_phrases.jsonl".into(), text.into_bytes()));

    // boosts
    let text = read_text(&paths.data("boosts.jsonl"))?;
    let mut boosts = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        let t = v["term"].as_str().unwrap_or("");
        let Some(id) = v["id"].as_str().and_then(FaceId::parse_any) else { continue };
        if t.is_empty() {
            continue;
        }
        let b = v.get("boost").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32;
        boosts.push(BoostIn { term: t.to_string(), id, boost: b });
    }
    digest.push(("boosts.jsonl".into(), text.into_bytes()));

    // situations
    let text = read_text(&paths.situations)?;
    let mut situations = Vec::new();
    if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&text) {
        for (k, s) in o {
            if k.starts_with('_') {
                continue;
            }
            situations.push(SituationIn {
                name: k.clone(),
                matches: strs(&s, "match"),
                p: profile(&emotions, s.get("p")),
                pair: s.get("pair").and_then(|x| x.as_bool()),
                words: strs(&s, "words"),
            });
        }
    }
    digest.push(("situations.json".into(), text.into_bytes()));

    // grammar
    let mut grammar = Vec::new();
    for p in paths.grammar_files() {
        let text = read_text(&p)?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", p.display())))?;
        let lang = v["lang"].as_str().map(String::from).unwrap_or_else(|| {
            p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        });
        let mut g = Grammar::new(&lang);
        for name in emoticond::grammar::LISTS {
            if let Some(l) = g.list_mut(name) {
                *l = strs(&v, name);
            }
        }
        digest.push((format!("grammar/{lang}"), text.into_bytes()));
        grammar.push(g);
    }
    if grammar.is_empty() {
        return Err(bad(format!("no grammar word lists in {}", paths.grammar_dir().display())));
    }

    // renamed and retired ids (optional)
    let aliases = match std::fs::read_to_string(&paths.aliases) {
        Ok(text) => {
            let a = parse_aliases(&text).map_err(|e| bad(format!("{}: {e}", paths.aliases.display())))?;
            digest.push(("aliases.jsonl".into(), text.into_bytes()));
            a
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io_err(&paths.aliases, e)),
    };

    let licence = read_text(&paths.data("licence/LICENCE.txt"))?;
    let attribution = read_text(&paths.data("licence/ATTRIBUTION.txt"))?;
    digest.push(("LICENCE".into(), licence.clone().into_bytes()));
    digest.push(("ATTRIBUTION".into(), attribution.clone().into_bytes()));

    let parts: Vec<&[u8]> = digest.iter().flat_map(|(n, b)| [n.as_bytes(), b.as_slice()]).collect();
    let source_rev = crate::sha256_hex(&parts)[..16].to_string();

    Ok(Sources {
        params,
        emotions,
        extras,
        faces,
        vocab,
        dense_k,
        dense,
        engine,
        phrases,
        situations,
        canonical,
        boosts,
        grammar,
        licence,
        attribution,
        source_rev,
        aliases,
    })
}

/// The alias file: JSON lines, one per id that left the data.
///
/// ```text
/// {"old": "k0123456789ab", "new": "kba9876543210", "why": "dialogue stripped"}
/// {"old": "k00000000beef", "retired": "licence"}
/// ```
///
/// `new` renames (`ALIA`); without it the id is retired (`RETD`) for the
/// reason in `retired`: `pruned`, `licence`, `blocklisted`, `quality` or
/// `other`. Blank lines and lines starting with `#` are skipped; anything
/// else unreadable is an error (a typo must not silently drop an alias).
pub fn parse_aliases(text: &str) -> Result<Vec<AliasIn>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |m: &str| format!("line {}: {m}", n + 1);
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| at(&e.to_string()))?;
        let id = |k: &str| -> Result<Option<FaceId>, String> {
            match v.get(k).and_then(|x| x.as_str()) {
                None => Ok(None),
                Some(s) => FaceId::parse_any(s).map(Some).ok_or_else(|| at(&format!("{k}: {s:?} is not a face id"))),
            }
        };
        let old = id("old")?.ok_or_else(|| at("no \"old\" id"))?;
        let new = id("new")?;
        let reason = match (new, v.get("retired").and_then(|x| x.as_str())) {
            (Some(_), None) => RETIRED_OTHER,
            (Some(_), Some(_)) => return Err(at("both \"new\" and \"retired\"")),
            (None, Some("pruned")) => RETIRED_PRUNED,
            (None, Some("licence" | "license")) => RETIRED_LICENCE,
            (None, Some("blocklisted")) => RETIRED_BLOCKLISTED,
            (None, Some("quality")) => RETIRED_QUALITY,
            (None, Some("other")) => RETIRED_OTHER,
            (None, Some(r)) => return Err(at(&format!("unknown reason {r:?}"))),
            (None, None) => return Err(at("neither \"new\" nor \"retired\"")),
        };
        out.push(AliasIn { old, new, reason });
    }
    Ok(out)
}

/// A selection list (`work/sets/<policy>/core.txt`): one FaceId per line
/// (`k` + 12 hex). Blank lines and `#` comments are skipped.
pub fn read_selection(path: &Path) -> Result<std::collections::HashSet<u64>, CompileError> {
    let text = read_text(path)?;
    let mut out = std::collections::HashSet::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let id = FaceId::parse_any(line)
            .ok_or_else(|| CompileError::Invalid(format!("{}:{}: {line:?} is not a face id", path.display(), n + 1)))?;
        out.insert(id.as_u64());
    }
    Ok(out)
}
