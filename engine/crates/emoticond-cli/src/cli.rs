//! The `emoticond` command line (docs/api-frontends.md §2).
//!
//! Every per-query option is a flag (options.md §10.3): `--safety moderate`,
//! `--lenny hide`, `--min-quality 4`, `--faces-only`, `--no-pinned`,
//! `--emotion sad=0.3`, ... -- any setting name works as `--name VALUE`,
//! dots or dashes alike. They sit at the command-line layer of the config
//! (above env and files, below the policy). `--opts JSON` takes per-query
//! options as the protocol's `opts` does.
//!
//! Exit codes (§2.5): 0 ok, 1 no results, 2 usage error, 3 no data,
//! 4 state/IO error or a missing tool, 5 refused by policy, 130 cancelled.

use super::app::{load_config, now_ms, random_id, App, ConfigArgs};
use super::out::{json, EntryOut, Fields, HitOut, SearchOut};
use emoticond::{Database, Entry, FaceId, Pick, Reason, Report, ReportBuilder, SearchOptions, SearchResult, Target, TermSource, Warning};
use emoticond_config::{canonical_key, setting, Value as CfgValue};
use emoticond_state::{PickLogRecord, DISCLAIMER_VERSION};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

pub const OK: i32 = 0;
pub const NO_RESULTS: i32 = 1;
pub const USAGE: i32 = 2;
pub const NO_DATA: i32 = 3;
pub const IO: i32 = 4;
/// Reserved: nothing is refused by policy yet (reports are always queued;
/// the policy can only turn sending off).
#[allow(dead_code)]
pub const POLICY: i32 = 5;
pub const CANCELLED: i32 = 130;

const HELP: &str = "\
emoticond -- a (legitimately) clever kaomoji search engine

usage:
  emoticond search [QUERY...|-] [OPTIONS] [--format plain|dmenu|tsv|json|jsonl|alfred]
  emoticond explain QUERY [--json] [--full]
  emoticond get ID|TEXT... [--json]
  emoticond similar ID|TEXT [--format F]
  emoticond complete PREFIX [-n N]
  emoticond browse [--recent|--starter] [--format F]
  emoticond pick ID|TEXT [--query Q] [--rank N]
  emoticond report --reason R [--face ID|TEXT] [--query Q] [--note TEXT] [--json]
  emoticond block ID|TEXT        emoticond unblock ID|TEXT
  emoticond menu [--launcher L] [--action copy|type|print] [OPTIONS]
  emoticond serve [--idle SECS] [--frontend NAME]      (stdio protocol, docs/protocol.md)
  emoticond config [show]        emoticond info [--json]
  emoticond reports [on|off]     (send the reports you make? shows the choice without on/off)
  emoticond data fetch [X.Y] [--set core|full|lite]   (download data into ~/.local/share/emoticond)

report reasons:
  query menu: read_well, read_wrong, missing, note
  face menu:  great_fit, other_word, no_fit, offensive (hides it), note
  either:     clear (withdraws your earlier report on the same query and face)

options (any setting works as --name VALUE; see `emoticond config show`):
  -n, --limit N        --safety strict|moderate|off   --lenny/--crude/--long hide|demote|allow
  --figures auto|single|pair|any   --min-quality Q   --max-len N   --faces-only
  --emotion NAME=V     --emotion-min NAME=V   --emotion-max NAME=V   --variety X --seed N
  --offset N   --exclude ID,ID   --no-pinned   --no-correct   --explain off|reading|full
  --fields flags,emotions,quality,attrs,canonical_for   (json/jsonl output)
  --opts JSON   --strict-options   --set KEY=VALUE
  --config PATH   --no-config   --profile NAME
  --json (= --format json)   --dmenu (= --format dmenu)

exit codes: 0 ok, 1 no results, 2 usage, 3 no data, 4 state/IO or missing tool,
            5 refused by policy, 130 cancelled (menu)
";

/// Parsed command line.
#[derive(Debug, Default)]
pub struct Args {
    pub cfg: ConfigArgs,
    /// `--opts` plus `--offset`, `--seed`, `--exclude`: a request layer.
    pub opts: Map<String, Value>,
    pub strict: bool,
    pub format: Option<String>,
    pub fields: Vec<String>,
    pub limit: Option<String>,
    pub pos: Vec<String>,
    pub values: BTreeMap<&'static str, String>,
    pub switches: BTreeSet<&'static str>,
}

impl Args {
    pub fn value(&self, k: &str) -> Option<&str> {
        self.values.get(k).map(String::as_str)
    }
    pub fn has(&self, k: &str) -> bool {
        self.switches.contains(k)
    }
}

/// Command-specific flags: (name, takes a value).
fn command_flags(cmd: &str) -> &'static [(&'static str, bool)] {
    match cmd {
        "explain" => &[("--full", false)],
        "browse" => &[("--recent", false), ("--popular", false), ("--starter", false)],
        "pick" => &[("--query", true), ("--rank", true)],
        "report" => &[("--reason", true), ("--face", true), ("--query", true), ("--note", true), ("--rank", true), ("--clears", true)],
        "serve" => &[("--idle", true), ("--socket", true), ("--frontend", true)],
        "menu" => &[("--launcher", true), ("--action", true), ("--prompt", true)],
        "data" => &[("--set", true), ("--dir", true)],
        _ => &[],
    }
}

fn is_bool_word(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "1" | "0")
}

fn setting_is_bool(key: &str) -> bool {
    setting(key).is_some_and(|s| matches!(s.default, CfgValue::Bool(_)))
}

/// Parse `args` (after the command name).
pub fn parse(cmd: &str, args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let specific = command_flags(cmd);
    let mut emotions: [Vec<String>; 3] = Default::default();
    let mut i = 0;
    let mut only_pos = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if only_pos || arg == "-" || !(arg.starts_with("--") || matches!(arg.as_str(), "-n" | "-h")) {
            a.pos.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_pos = true;
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut need = |what: &str| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            let v = args.get(i).cloned().ok_or_else(|| format!("{name} needs {what}"))?;
            i += 1;
            Ok(v)
        };
        if let Some((n, takes)) = specific.iter().find(|(n, _)| *n == name) {
            if *takes {
                let v = need("a value")?;
                a.values.insert(n, v);
            } else {
                a.switches.insert(n);
            }
            continue;
        }
        match name.as_str() {
            "-h" | "--help" => {
                a.switches.insert("--help");
            }
            "--config" => a.cfg.config = Some(need("a path")?.into()),
            "--no-config" => a.cfg.no_config = true,
            "--profile" => a.cfg.profile = Some(need("a name")?),
            "--set" => a.cfg.sets.push(need("key=value")?),
            "--opts" => {
                let v: Value = serde_json::from_str(&need("a JSON object")?).map_err(|e| format!("--opts: {e}"))?;
                let Value::Object(m) = v else { return Err("--opts must be a JSON object".into()) };
                a.opts.extend(m);
            }
            "--strict-options" => a.strict = true,
            "--format" => a.format = Some(need("a format")?),
            "--json" => {
                a.format = Some("json".into());
                a.switches.insert("--json");
            }
            "--dmenu" => a.format = Some("dmenu".into()),
            "--fields" => a.fields.extend(need("a list")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty())),
            "-n" | "--limit" => {
                let v = need("a number")?;
                a.limit = Some(v.clone());
                a.cfg.sets.push(format!("search.limit={v}"));
            }
            "--emotion" => emotions[0].push(need("NAME=VALUE")?),
            "--emotion-min" => emotions[1].push(need("NAME=VALUE")?),
            "--emotion-max" => emotions[2].push(need("NAME=VALUE")?),
            "--offset" | "--seed" => {
                let v = need("a number")?;
                let n: u64 = v.parse().map_err(|_| format!("{name} needs a non-negative integer, got {v:?}"))?;
                a.opts.insert(name.trim_start_matches("--").into(), json!(n));
            }
            "--exclude" => {
                let ids: Vec<Value> = need("face ids")?.split(',').map(|s| json!(s.trim())).collect();
                a.opts.insert("exclude".into(), Value::Array(ids));
            }
            "--lenny" | "--crude" | "--long" => {
                let v = need("hide, demote or allow")?;
                a.cfg.sets.push(format!("search.styles.{}={v}", name.trim_start_matches("--")));
            }
            _ => {
                let raw = name.trim_start_matches("--");
                // `--feedback-send` for `feedback.send`
                let key = |r: &str| canonical_key(r).or_else(|| canonical_key(&r.replacen('-', ".", 1)));
                if let Some(k) = raw.strip_prefix("no-").and_then(key).filter(|k| setting_is_bool(k)) {
                    if inline.is_some() {
                        return Err(format!("{name} takes no value"));
                    }
                    a.cfg.sets.push(format!("{k}=false"));
                    continue;
                }
                let Some(k) = key(raw) else { return Err(format!("unknown option {name}")) };
                let v = if setting_is_bool(k) {
                    match inline.clone() {
                        Some(v) => v,
                        None if args.get(i).is_some_and(|n| is_bool_word(n)) => {
                            i += 1;
                            args[i - 1].clone()
                        }
                        None => "true".into(),
                    }
                } else {
                    need("a value")?
                };
                a.cfg.sets.push(format!("{k}={v}"));
            }
        }
    }
    for (key, list) in ["search.emotions.target", "search.emotions.min", "search.emotions.max"].iter().zip(emotions) {
        if !list.is_empty() {
            a.cfg.sets.push(format!("{key}={}", list.join(",")));
        }
    }
    Ok(a)
}

pub fn main(args: &[String]) -> i32 {
    let Some(cmd) = args.get(1) else {
        eprint!("{HELP}");
        return USAGE;
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            OK
        }
        "-V" | "--version" | "version" => {
            println!("emoticond {} (protocol {})", emoticond::VERSION, super::proto::PROTOCOL);
            OK
        }
        "search" | "explain" | "get" | "similar" | "complete" | "browse" | "pick" | "report" | "block" | "unblock" | "serve" | "menu"
        | "config" | "info" | "data" | "reports" => {
            let a = match parse(cmd, &args[2..]) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("emoticond {cmd}: {e}");
                    return USAGE;
                }
            };
            if a.has("--help") {
                print!("{HELP}");
                return OK;
            }
            run(cmd, a)
        }
        other => {
            eprintln!("emoticond: unknown command {other:?} (see emoticond --help)");
            USAGE
        }
    }
}

fn warn(w: &Warning) {
    let level = match emoticond_config::level(w) {
        emoticond_config::Level::Info => "note",
        emoticond_config::Level::Warning => "warning",
        emoticond_config::Level::Error => "error",
    };
    eprintln!("emoticond: {level}: {}", w.message);
}

/// The front-end name for a command's `[profile.<name>]`.
fn frontend(cmd: &str) -> &'static str {
    match cmd {
        "menu" => "menu",
        "serve" => "serve",
        _ => "cli",
    }
}

fn open_app(cmd: &str, a: &Args) -> Result<App, i32> {
    let cfg = load_config(frontend(cmd), &a.cfg).map_err(|e| {
        eprintln!("emoticond: {e}");
        USAGE
    })?;
    let app = App::new(cfg);
    for w in app.warnings() {
        warn(&w);
    }
    Ok(app)
}

/// The data, through main.rs (the one place that knows how to open it).
fn open_db(cfg: &emoticond_config::Resolved) -> Result<Database, i32> {
    let b = super::build(cfg);
    b.db.ok_or_else(|| {
        if b.failed {
            eprintln!("emoticond: run `emoticond data fetch` for data this version can read");
            return NO_DATA;
        }
        let looked: Vec<String> = b.searched.iter().map(|p| p.display().to_string()).collect();
        eprintln!(
            "emoticond: no kaomoji data found (looked for {}); run `emoticond data fetch` to download it, or set EMOTICOND_DATA_FILE to a .kmj",
            looked.join(", ")
        );
        NO_DATA
    })
}

/// The per-query options: config, then `--opts`/`--offset`/..., then usage
/// and the blocklist.
fn options(app: &App, a: &Args) -> Result<SearchOptions, i32> {
    let keep_usage = a.opts.contains_key("usage");
    let (mut o, w) = app.options_with(&a.opts).map_err(|e| {
        eprintln!("emoticond: {e}");
        USAGE
    })?;
    for x in &w {
        warn(x);
    }
    if a.strict && w.iter().any(|w| w.code == "unknown_key" || w.code == "obsolete_key") {
        eprintln!("emoticond: unknown option in --opts (--strict-options)");
        return Err(USAGE);
    }
    app.finish(&mut o, keep_usage, None);
    Ok(o)
}

/// A face named by id (`k…`, or 12 hex digits) or by exact text.
pub fn resolve(db: &Database, s: &str) -> Option<Entry> {
    match FaceId::parse_any(s) {
        Some(id) => db.get(id),
        None => db.find_text(s),
    }
}

const CONSENT_QUESTION: &str = "\
emoticond: one question, asked once.

  the report menus in pickers (and `emoticond report`) let you say a search
  was read wrong, a face doesn't fit, or a face is offensive. those reports
  can be sent to improve the data for everyone. a report has your search,
  how it was read, the face and the top 20 faces shown; nothing else
  (no id, no history). nothing is sent unless you say yes, and only reports
  made after you say yes. change it any time: emoticond reports on|off

send reports? [y/n] ";

/// The first interactive run asks "send reports?" (docs/collector.md), on
/// a terminal only: a keybind, a script or a front-end never blocks here.
/// There is no default: anything but y/yes/n/no asks again, and end of
/// input leaves it unanswered (nothing is sent).
fn ask_consent_once(app: &App) {
    use std::io::{BufRead, IsTerminal, Write};
    if !cfg!(feature = "net") || app.sending().off_by != Some("unasked") {
        return;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return;
    }
    let mut err = std::io::stderr();
    let _ = write!(err, "{CONSENT_QUESTION}");
    let mut line = String::new();
    loop {
        let _ = err.flush();
        line.clear();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            let _ = writeln!(err);
            return;
        }
        let send = match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => {
                let _ = write!(err, "y or n: ");
                continue;
            }
        };
        match app.set_consent(send) {
            Ok(()) => {
                let _ = writeln!(err, "{}\n", if send { "thanks! reports will be sent." } else { "ok, reports stay on this computer." });
            }
            Err(e) => {
                let _ = writeln!(err, "emoticond: {e}\n");
            }
        }
        return;
    }
}

/// `emoticond reports [on|off]`: save the answer, or show where it stands.
fn reports(app: &App, a: &Args) -> i32 {
    let send = match a.pos.first().map(|s| s.to_ascii_lowercase()) {
        None => None,
        Some(w) if matches!(w.as_str(), "on" | "yes" | "y" | "true") => Some(true),
        Some(w) if matches!(w.as_str(), "off" | "no" | "n" | "false") => Some(false),
        Some(w) => {
            eprintln!("emoticond reports: on or off, not {w:?}");
            return USAGE;
        }
    };
    if let Some(send) = send {
        if let Err(e) = app.set_consent(send) {
            eprintln!("emoticond reports: {e}");
            return POLICY;
        }
    }
    let s = app.sending();
    println!(
        "reports: {}",
        match s.off_by {
            None => "sent (emoticond reports off to stop)".to_string(),
            Some("unasked") => "not chosen yet, so not sent (emoticond reports on|off)".to_string(),
            Some("policy") => "not sent: turned off by your system administrator".to_string(),
            Some(_) => "not sent (emoticond reports on to send them)".to_string(),
        }
    );
    OK
}

fn run(cmd: &str, a: Args) -> i32 {
    match cmd {
        "config" => return config(&a),
        "data" => return super::fetch::data(&a),
        "serve" => {
            if a.value("--socket").is_some() {
                eprintln!("emoticond serve: --socket is not supported yet (protocol v1 is stdio only)");
                return USAGE;
            }
            let idle = match a.value("--idle").map(str::parse::<u64>) {
                None => None,
                Some(Ok(n)) => Some(n),
                Some(Err(_)) => {
                    eprintln!("emoticond serve: --idle needs a number of seconds");
                    return USAGE;
                }
            };
            let fe = a.value("--frontend").unwrap_or("serve").to_string();
            return super::proto::serve(&fe, &a.cfg, idle);
        }
        _ => {}
    }
    let mut app = match open_app(cmd, &a) {
        Ok(x) => x,
        Err(c) => return c,
    };
    if cmd == "reports" {
        return reports(&app, &a);
    }
    if cmd != "menu" {
        ask_consent_once(&app);
    }
    let db = match open_db(&app.cfg) {
        Ok(d) => d,
        Err(c) => return c,
    };
    for w in app.load_overlays(Some(&db)).into_iter().filter(|w| w.level != emoticond::Level::Info) {
        warn(&w);
    }
    let r = match cmd {
        "search" => search(&db, &app, &a),
        "explain" => explain(&db, &app, &a),
        "get" => get(&db, &a),
        "similar" => similar(&db, &app, &a),
        "complete" => complete(&db, &a),
        "browse" => browse(&db, &app, &a),
        "pick" => pick(&db, &mut app, &a),
        "report" => report(&db, &mut app, &a),
        "block" | "unblock" => block(&db, &mut app, &a, cmd == "block"),
        "menu" => super::menu::run(&db, &mut app, &a),
        "info" => info(&db, &app, &a),
        _ => Ok(USAGE),
    };
    r.unwrap_or_else(|c| c)
}

fn format(a: &Args) -> Result<&str, i32> {
    let f = a.format.as_deref().unwrap_or("plain");
    match f {
        "plain" | "dmenu" | "tsv" | "json" | "jsonl" | "alfred" => Ok(f),
        other => {
            eprintln!("emoticond: unknown format {other:?} (plain, dmenu, tsv, json, jsonl, alfred)");
            Err(USAGE)
        }
    }
}

/// Print a result in the chosen format; exit 1 when it is empty.
fn print_result(db: &Database, q: &str, mode: &'static str, r: &SearchResult, a: &Args, t0: std::time::Instant) -> Result<i32, i32> {
    let fmt = format(a)?;
    for w in &r.warnings {
        warn(w);
    }
    let mut fw = Vec::new();
    let fields = Fields::parse(a.fields.iter().map(String::as_str), &mut fw);
    for w in &fw {
        warn(w);
    }
    match fmt {
        "plain" => {
            if let Some(rd) = r.reading.as_ref().filter(|rd| !rd.line.is_empty()) {
                eprintln!("read as: {}", rd.line);
            }
            if let Some(c) = &r.corrected {
                eprintln!("showing results for: {c}");
            }
            for h in &r.hits {
                println!("{}", h.text);
            }
        }
        "dmenu" => {
            for h in &r.hits {
                println!("{}\t{}\t{}", h.text, HitOut::hint(db, h.id), h.id);
            }
        }
        "tsv" => {
            for h in &r.hits {
                println!("{}\temoticon\t{}\t{:.4}\t", h.id, h.text, h.score);
            }
        }
        "json" => {
            let mut o = SearchOut::of(db, q, mode, r, fields, Vec::new());
            o.ms = (t0.elapsed().as_secs_f64() * 1e4).round() / 10.0;
            println!("{}", json(&o))
        }
        "jsonl" => {
            for h in &r.hits {
                println!("{}", json(&HitOut::of(db, h, fields, mode == "browse")));
            }
        }
        "alfred" => {
            let items: Vec<Value> = r
                .hits
                .iter()
                .map(|h| json!({"uid": h.id.to_string(), "title": h.text, "subtitle": HitOut::hint(db, h.id), "arg": h.text}))
                .collect();
            println!("{}", json(&json!({ "items": items })));
        }
        _ => unreachable!(),
    }
    Ok(if r.hits.is_empty() { NO_RESULTS } else { OK })
}

fn query_text(a: &Args) -> Result<String, i32> {
    if a.pos.len() == 1 && a.pos[0] == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| {
            eprintln!("emoticond: reading the query from stdin: {e}");
            IO
        })?;
        return Ok(s.trim().to_string());
    }
    Ok(a.pos.join(" "))
}

fn search(db: &Database, app: &App, a: &Args) -> Result<i32, i32> {
    let t0 = std::time::Instant::now();
    let q = query_text(a)?;
    let o = options(app, a)?;
    if q.trim().is_empty() {
        let r = db.browse(&o);
        return print_result(db, "", "browse", &r, a, t0);
    }
    let r = db.search(&q, &o);
    print_result(db, &q, "search", &r, a, t0)
}

fn browse(db: &Database, app: &App, a: &Args) -> Result<i32, i32> {
    let t0 = std::time::Instant::now();
    let mut o = options(app, a)?;
    if a.has("--recent") || a.has("--popular") {
        o.pinned = false;
    }
    if a.has("--starter") {
        o.usage = Default::default();
    }
    let r = db.browse(&o);
    print_result(db, "", "browse", &r, a, t0)
}

fn explain(db: &Database, app: &App, a: &Args) -> Result<i32, i32> {
    let q = query_text(a)?;
    if q.trim().is_empty() {
        eprintln!("emoticond explain: give a query");
        return Err(USAGE);
    }
    let mut o = options(app, a)?;
    o.explain = if a.has("--full") { emoticond::Explain::Full } else { emoticond::Explain::Reading };
    let r = db.read(&q, &o);
    if a.has("--json") {
        println!("{}", json(&json!({"q": q, "reading": r})));
    } else {
        println!("{}", r.line);
        if let Some(d) = &r.debug {
            println!("{d}");
        }
    }
    Ok(OK)
}

fn get(db: &Database, a: &Args) -> Result<i32, i32> {
    if a.pos.is_empty() {
        eprintln!("emoticond get: give face ids or texts");
        return Err(USAGE);
    }
    let found: Vec<Option<Entry>> = a.pos.iter().map(|p| resolve(db, p)).collect();
    for (p, e) in a.pos.iter().zip(&found) {
        if e.is_none() {
            eprintln!("emoticond: no face {p:?}");
        }
    }
    if a.has("--json") {
        let v: Vec<Option<EntryOut>> = found.iter().map(|e| e.as_ref().map(EntryOut::of)).collect();
        println!("{}", json(&v));
    } else {
        for e in found.iter().flatten() {
            let emo: Vec<String> = e.top_emotions(3).iter().map(|(k, v)| format!("{k} {v:.2}")).collect();
            let canon = if e.canonical_for.is_empty() { String::new() } else { format!("\tcanonical: {}", e.canonical_for.join(", ")) };
            println!("{}\t{}\tquality {:.1}\t{}{}", e.id, e.text, e.quality, emo.join(", "), canon);
        }
    }
    Ok(if found.iter().any(Option::is_some) { OK } else { NO_RESULTS })
}

fn similar(db: &Database, app: &App, a: &Args) -> Result<i32, i32> {
    let t0 = std::time::Instant::now();
    let Some(face) = a.pos.first() else {
        eprintln!("emoticond similar: give a face id or text");
        return Err(USAGE);
    };
    let Some(e) = resolve(db, face) else {
        eprintln!("emoticond: no face {face:?}");
        return Ok(NO_RESULTS);
    };
    let o = options(app, a)?;
    let r = db.similar(e.id, &o);
    print_result(db, &e.text, "similar", &r, a, t0)
}

fn complete(db: &Database, a: &Args) -> Result<i32, i32> {
    let prefix = a.pos.join(" ");
    let n = a.limit.as_deref().and_then(|l| l.parse().ok()).unwrap_or(10usize);
    let c = db.complete(&prefix, n);
    if a.has("--json") {
        println!("{}", json(&c));
    } else {
        for x in &c {
            println!("{}", x.text);
        }
    }
    Ok(if c.is_empty() { NO_RESULTS } else { OK })
}

/// Record a pick: local popularity and the dev pick log. Shared with `menu`.
pub fn record_pick(db: &Database, app: &mut App, e: &Entry, q: &str, o: &SearchOptions, rank: Option<u32>) -> Result<i32, i32> {
    let (term_key, shipped, shown, pos) = if q.trim().is_empty() {
        (None, false, Vec::new(), None)
    } else {
        let r = db.search(q, o);
        let pos = r.hits.iter().position(|h| h.id == e.id).map(|p| p as u32);
        (r.term_key.clone(), r.term_source == TermSource::Shipped, r.hits.iter().map(|h| h.text.clone()).collect(), pos)
    };
    let mut rec = PickLogRecord::default();
    rec.q = q.to_string();
    rec.text = e.text.clone();
    rec.rank = rank.or(pos).unwrap_or(0);
    rec.shown = shown;
    rec.kind = Some("emoticon".into());
    let (recorded, _logged, err) = app.pick(&Pick::new(e.id, term_key), shipped, Some(&rec));
    if let Some(err) = err {
        eprintln!("emoticond: could not record the pick: {err}");
        return Err(IO);
    }
    if !recorded && app.popularity() == emoticond::PopularityMode::Off {
        eprintln!("emoticond: note: popularity is off; the pick was not recorded");
    }
    Ok(OK)
}

fn pick(db: &Database, app: &mut App, a: &Args) -> Result<i32, i32> {
    let Some(face) = a.pos.first() else {
        eprintln!("emoticond pick: give a face id or text");
        return Err(USAGE);
    };
    let Some(e) = resolve(db, face) else {
        eprintln!("emoticond: no face {face:?}");
        return Ok(NO_RESULTS);
    };
    let rank = match a.value("--rank").map(str::parse::<u32>) {
        None => None,
        Some(Ok(r)) => Some(r),
        Some(Err(_)) => {
            eprintln!("emoticond pick: --rank needs a number");
            return Err(USAGE);
        }
    };
    let o = options(app, a)?;
    record_pick(db, app, &e, a.value("--query").unwrap_or(""), &o, rank)
}

fn report(db: &Database, app: &mut App, a: &Args) -> Result<i32, i32> {
    let Some(reason) = a.value("--reason") else {
        eprintln!("emoticond report: --reason is required (read_well, read_wrong, missing, great_fit, other_word, no_fit, offensive, note, clear)");
        return Err(USAGE);
    };
    let Ok(reason) = serde_json::from_value::<Reason>(json!(reason.replace('-', "_"))) else {
        eprintln!("emoticond report: unknown reason {reason:?}");
        return Err(USAGE);
    };
    let face = match a.value("--face").or(a.pos.first().map(String::as_str)) {
        Some(f) => match resolve(db, f) {
            Some(e) => Some(e),
            None => {
                eprintln!("emoticond: no face {f:?}");
                return Ok(NO_RESULTS);
            }
        },
        None => None,
    };
    let q = a.value("--query").unwrap_or("");
    if reason.needs_face() && face.is_none() {
        eprintln!("emoticond report: {reason:?} is about one face: give --face");
        return Err(USAGE);
    }
    if reason.needs_query() && q.trim().is_empty() {
        eprintln!("emoticond report: {reason:?} is about a search: give --query");
        return Err(USAGE);
    }
    let rank = a.value("--rank").and_then(|r| r.parse::<u32>().ok());
    let mut o = options(app, a)?;
    if o.explain == emoticond::Explain::Off {
        o.explain = emoticond::Explain::Reading;
    }
    let r = if q.trim().is_empty() { SearchResult::default() } else { db.search(q, &o) };
    let b = match &face {
        Some(e) => match r.hits.iter().find(|h| h.id == e.id) {
            Some(h) if rank.is_none() => Report::for_face(&r, q, h, reason),
            hit => ReportBuilder::new(q, Target::Face { id: e.id, text: e.text.clone(), rank: rank.or(hit.map(|h| h.rank)).unwrap_or(0) }, reason)
                .term_key(r.term_key.clone())
                .reading(r.reading.as_ref().map(|x| x.line.clone()))
                .in_force(r.safety, r.styles)
                .shown(r.hits.iter().map(|h| h.id)),
        },
        None => Report::for_query(&r, q, reason),
    };
    let mut b = b
        .max_note_chars(usize::from(app.cfg.config.feedback.custom_max_chars))
        .stamp(random_id(), now_ms(), DISCLAIMER_VERSION)
        .data_version(db.info().data_version);
    if let Some(n) = a.value("--note") {
        b = b.note(n);
    }
    if reason == Reason::Clear {
        let named = a.value("--clears").and_then(|c| serde_json::from_value::<Reason>(serde_json::Value::String(c.into())).ok());
        if let Some(c) = named.or_else(|| app.last_choice(b.query(), b.target())) {
            b = b.clears(c);
        }
    }
    let effects = b.local_effects();
    let report = b.build().map_err(|e| {
        eprintln!("emoticond report: {e}");
        USAGE
    })?;
    let sending = app.sending();
    let s = app.submit(&report, &effects).map_err(|e| {
        eprintln!("emoticond: could not queue the report: {e}");
        IO
    })?;
    if let Some(e) = &s.error {
        eprintln!("emoticond: warning: {e}");
    }
    if a.has("--json") {
        println!("{}", super::proto::report_ack(None, sending, &s));
    } else {
        let fx = if s.effects.is_empty() { String::new() } else { format!(": {}", s.effects.join(", ")) };
        println!("report queued ({}){fx}", serde_json::to_value(reason).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default());
        println!("{}", sending.footer());
    }
    Ok(OK)
}

fn block(db: &Database, app: &mut App, a: &Args, on: bool) -> Result<i32, i32> {
    let Some(face) = a.pos.first() else {
        eprintln!("emoticond: give a face id or text");
        return Err(USAGE);
    };
    let (id, text) = match resolve(db, face) {
        Some(e) => (e.id, Some(e.text)),
        None => match FaceId::parse_any(face) {
            Some(id) if !on => (id, None),
            _ => {
                eprintln!("emoticond: no face {face:?}");
                return Ok(NO_RESULTS);
            }
        },
    };
    let r = if on { app.block(id, text.as_deref()) } else { app.unblock(id) };
    match r {
        Ok(changed) => {
            let what = match (on, changed) {
                (true, true) => "blocked",
                (true, false) => "already blocked",
                (false, true) => "unblocked",
                (false, false) => "was not blocked",
            };
            println!("{what}: {id} {}", text.unwrap_or_default());
            Ok(OK)
        }
        Err(e) => {
            eprintln!("emoticond: {e}");
            Err(IO)
        }
    }
}

fn config(a: &Args) -> i32 {
    match a.pos.first().map(String::as_str) {
        None | Some("show") => {}
        Some(other) => {
            eprintln!("emoticond config: unknown subcommand {other:?} (show)");
            return USAGE;
        }
    }
    match load_config(frontend("cli"), &a.cfg) {
        Ok(c) => {
            print!("{}", c.describe());
            OK
        }
        Err(e) => {
            eprintln!("emoticond: {e}");
            USAGE
        }
    }
}

fn info(db: &Database, app: &App, a: &Args) -> Result<i32, i32> {
    let i = db.info();
    let mut defaults = app.cfg.search.clone();
    defaults.usage = Default::default();
    let p = &app.paths;
    let sending = app.sending();
    let v = json!({
        "engine": i.engine_version,
        "protocol": super::proto::PROTOCOL,
        "data": {"path": i.path, "dir": i.data_dir, "format": i.format, "version": i.data_version, "dataset": i.dataset,
                 "licence": i.licence, "content_hash": i.content_hash, "bytes": i.bytes, "faces": i.faces, "terms": i.terms, "phrases": i.phrases,
                 "situations": i.situations, "canonical": i.canonical, "boosts": i.boosts},
        "emotions": i.emotions,
        "profile": app.cfg.profile,
        "config_files": app.cfg.files,
        "state": {"dir": p.root, "usage": p.usage, "blocklist": p.blocklist, "queue": p.queue, "boosts": p.boosts},
        "popularity": app.popularity().as_str(),
        "sending": sending.on,
        "dev": {"pick_log": app.pick_log.path()},
        "defaults": defaults,
    });
    if a.has("--json") {
        println!("{}", json(&v));
        return Ok(OK);
    }
    println!("emoticond {} (protocol {})", i.engine_version, super::proto::PROTOCOL);
    println!("data:        {} ({} {}, {}, {} faces, {} terms, {} phrases, {} canonical)", i.path, i.dataset, i.data_version, i.format, i.faces, i.terms, i.phrases, i.canonical);
    println!("licence:     {}", i.licence);
    println!("profile:     {}", app.cfg.profile.as_deref().unwrap_or("(none)"));
    let files: Vec<String> = app.cfg.files.iter().map(|f| f.display().to_string()).collect();
    println!("config:      {}", if files.is_empty() { "(none)".into() } else { files.join(", ") });
    println!("state:       {}", p.root.display());
    println!("  usage:     {}", p.usage.display());
    println!("  blocklist: {}", p.blocklist.display());
    println!("  reports:   {}", p.queue.display());
    println!("popularity:  {}", app.popularity().as_str());
    println!("reports:     {}", if sending.on { "sent once an endpoint exists (queued for now)" } else { "saved on this computer only" });
    let dev = |l: &emoticond_state::DevLog| l.path().map_or("off".to_string(), |p| p.display().to_string());
    println!("pick log:    {}", dev(&app.pick_log));
    println!("defaults:    {}", json(&defaults));
    Ok(OK)
}
