//! emoticond-compile: build a `.kmj` data file from today's private inputs.
//!
//!     emoticond-compile [--repo DIR] [--engine DIR] [-o FILE] [options]
//!
//! Inputs
//! --repo DIR          checkout holding data/ and emoticondb/ (default: .)
//! --engine DIR        the engine export (default: <repo>/work/engine)
//! --data DIR          curated data (default: <repo>/data)
//! --situations FILE   (default: <repo>/data/situations.json)
//! --aliases FILE      renamed/retired ids, JSONL (default: <data>/aliases.jsonl, if present)
//!
//! The data set
//! --set full|core|lite   (default: full). core and lite keep only the faces
//!                     of a selection list; lite also cuts the neighbour lists
//! --select FILE       the selection list, one FaceId per line (default for
//!                     core/lite: <engine>/../sets/<policy>/core.txt)
//! --neighbours K      keep the first K slots of every neighbour list
//!                     (default: 32 for lite, else all)
//! --policy private|public  public: refuse an export whose provenance labels
//!                     (wc, ev, meta.json policy) the public policy does not
//!                     allow; sets the header's public flag and
//!                     policy=public@<digest> (default: private)
//!
//! Storage (docs/format.md §5.3)
//! --encoding legacy|compact|small  format 3.0 as before / every lossless
//!                     saving (3.1, the default) / compact with 1-byte face
//!                     numbers (lossy)
//! --nums f32|q16|q8, --lists u32|packed, --postings u32|varint,
//! --chars stored|derived, --strings plain|dedupe   one choice at a time
//!
//! Manifest
//! --data-version V    data release (default: dev)
//! --build-date D      recorded as is (default: empty, for reproducible builds)
//! --source-rev R      recorded as is (default: a digest of the inputs)
//!
//! -o, --out FILE      output (default: <repo>/work/data/<set>.kmj)
//! --if-stale          do nothing when the output is newer than every input
//! --verify            open the result and check every section's checksum
//!
//! The output is written beside and then renamed into place.

use emoticond::FaceId;
use emoticond_compile::legacy::{read_selection, LegacyPaths};
use emoticond_compile::{compile, write_atomic, Encoding, ListEncoding, NumsEncoding, Params, PostingsEncoding};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn usage() -> ExitCode {
    eprintln!(
        "usage: emoticond-compile [--repo DIR] [--engine DIR] [--data DIR] [--situations FILE] [--aliases FILE] [-o FILE]\n\
         \x20      [--set full|core|lite] [--select FILE] [--neighbours K] [--policy private|public]\n\
         \x20      [--encoding legacy|compact|small] [--nums f32|q16|q8] [--lists u32|packed] [--postings u32|varint]\n\
         \x20      [--chars stored|derived] [--strings plain|dedupe]\n\
         \x20      [--data-version V] [--build-date D] [--source-rev R] [--if-stale] [--verify]"
    );
    ExitCode::from(2)
}

fn fail(m: impl std::fmt::Display) -> ExitCode {
    eprintln!("emoticond-compile: {m}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut repo, mut engine, mut data, mut situations, mut out) = (PathBuf::from("."), None, None, None, None);
    let (mut aliases, mut select, mut neighbours, mut source_rev): (Option<PathBuf>, Option<PathBuf>, Option<usize>, Option<String>) =
        (None, None, None, None);
    let mut params = Params::default();
    let mut public = false;
    let (mut if_stale, mut verify) = (false, false);
    while let Some(a) = args.next() {
        let Some(v) = (match a.as_str() {
            "--if-stale" => {
                if_stale = true;
                continue;
            }
            "--verify" => {
                verify = true;
                continue;
            }
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            _ => args.next(),
        }) else {
            return usage();
        };
        let enc = &mut params.encoding;
        let ok = match (a.as_str(), v.as_str()) {
            ("--repo", _) => {
                repo = PathBuf::from(&v);
                true
            }
            ("--engine", _) => {
                engine = Some(PathBuf::from(&v));
                true
            }
            ("--data", _) => {
                data = Some(PathBuf::from(&v));
                true
            }
            ("--situations", _) => {
                situations = Some(PathBuf::from(&v));
                true
            }
            ("--aliases", _) => {
                aliases = Some(PathBuf::from(&v));
                true
            }
            ("-o" | "--out", _) => {
                out = Some(PathBuf::from(&v));
                true
            }
            ("--set", "full" | "core" | "lite") => {
                params.dataset = v.clone();
                true
            }
            ("--select", _) => {
                select = Some(PathBuf::from(&v));
                true
            }
            ("--neighbours" | "--neighbors", _) => {
                neighbours = v.parse().ok().filter(|&k| k > 0);
                neighbours.is_some()
            }
            ("--policy", "private") => {
                public = false;
                true
            }
            ("--policy", "public") => {
                public = true;
                true
            }
            ("--encoding", _) => Encoding::named(&v).map(|e| *enc = e).is_some(),
            ("--nums", "f32") => {
                enc.nums = NumsEncoding::F32;
                true
            }
            ("--nums", "q16") => {
                enc.nums = NumsEncoding::Q16;
                true
            }
            ("--nums", "q8") => {
                enc.nums = NumsEncoding::Q8;
                true
            }
            ("--lists", "u32") => {
                enc.lists = ListEncoding::U32;
                true
            }
            ("--lists", "packed") => {
                enc.lists = ListEncoding::Packed;
                true
            }
            ("--postings", "u32") => {
                enc.postings = PostingsEncoding::U32;
                true
            }
            ("--postings", "varint") => {
                enc.postings = PostingsEncoding::Varint;
                true
            }
            ("--chars", "stored" | "derived") => {
                enc.store_chars = v == "stored";
                true
            }
            ("--strings", "plain" | "dedupe") => {
                enc.dedupe_strings = v == "dedupe";
                true
            }
            ("--data-version", _) => {
                params.data_version = v.clone();
                true
            }
            ("--build-date", _) => {
                params.build_date = v.clone();
                true
            }
            ("--source-rev", _) => {
                source_rev = Some(v.clone());
                true
            }
            _ => false,
        };
        if !ok {
            eprintln!("emoticond-compile: bad option {a} {v}");
            return usage();
        }
    }
    let policy_name = if public { "public" } else { "private" };
    if public {
        params.policy = emoticond_compile::policy::public_policy_id();
    }
    let engine = engine.unwrap_or_else(|| repo.join("work/engine"));
    let mut paths = LegacyPaths::for_repo(&repo, &engine);
    if let Some(d) = data {
        paths.aliases = d.join("aliases.jsonl");
        paths.data_dir = d;
    }
    if let Some(s) = situations {
        paths.situations = s;
    }
    if let Some(a) = aliases {
        paths.aliases = a;
    }
    let subset = params.dataset != "full";
    let select = select.or_else(|| subset.then(|| engine.join("..").join("sets").join(policy_name).join("core.txt")));
    let neighbours = neighbours.or((params.dataset == "lite").then_some(32));
    let out = out.unwrap_or_else(|| repo.join(format!("work/data/{}.kmj", params.dataset)));
    if if_stale && !paths.is_stale(&out) && select.as_ref().is_none_or(|s| !newer(s, &out)) {
        eprintln!("emoticond-compile: {} is up to date", out.display());
        return ExitCode::SUCCESS;
    }
    let t0 = Instant::now();
    let mut src = match paths.read(params) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let n_export = src.faces.len();
    if let Some(sel) = &select {
        let ids = match read_selection(sel) {
            Ok(ids) => ids,
            Err(e) => return fail(e),
        };
        let kept = src.retain_faces(|f| ids.contains(&FaceId::of_text(&f.text).as_u64()));
        if kept < ids.len() {
            eprintln!("emoticond-compile: {} of the {} ids in {} are not in the export", ids.len() - kept, ids.len(), sel.display());
        }
        eprintln!("emoticond-compile: set {}: {kept} of {n_export} faces", src.params.dataset);
    }
    if let Some(k) = neighbours {
        src.truncate_neighbours(k);
    }
    if let Some(r) = source_rev {
        src.source_rev = r;
    }
    let bytes = match compile(&src) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    let built = t0.elapsed();
    if let Err(e) = write_atomic(&out, &bytes) {
        return fail(format!("writing {}: {e}", out.display()));
    }
    let t1 = Instant::now();
    let db = match emoticond::Database::open(emoticond::OpenOptions::file(&out)) {
        Ok(db) => db,
        Err(e) => return fail(format!("the written file does not open: {e}")),
    };
    let opened = t1.elapsed();
    if verify {
        if let Err(tag) = db.verify() {
            return fail(format!("section {tag} fails its checksum"));
        }
    }
    let i = db.info();
    eprintln!(
        "emoticond-compile: {} (format {}, {}, {} bytes, {} faces, {} terms, k {}, {} phrases, {} situations, {} canonical, {} boosted terms; {}) in {:.2} s; opens in {:.2} ms",
        out.display(),
        i.format,
        db.manifest().policy,
        i.bytes,
        i.faces,
        i.terms,
        db.manifest().dense_k,
        i.phrases,
        i.situations,
        i.canonical,
        i.boosts,
        src.params.encoding.describe(),
        built.as_secs_f64(),
        opened.as_secs_f64() * 1000.0
    );
    ExitCode::SUCCESS
}

/// Whether `a` was modified after `b` (or `b` is missing).
fn newer(a: &std::path::Path, b: &std::path::Path) -> bool {
    let m = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (m(a), m(b)) {
        (Some(x), Some(y)) => x > y,
        _ => true,
    }
}
