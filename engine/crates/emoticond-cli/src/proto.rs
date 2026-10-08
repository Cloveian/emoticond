//! The stdio protocol: `emoticond serve` (docs/protocol.md,
//! docs/api-frontends.md §3).
//!
//! Lines are read on their own thread, so before each request the daemon
//! knows every line already sent: a search on a channel (`"chan"`) that a
//! later queued search on the same channel replaces is answered at once with
//! `superseded`, and `cancel` drops a queued request.

use super::app::{load_config, now_ms, random_id, App, ConfigArgs};
use super::app::{Sending, Submitted};
use super::out::{json, EntryOut, ErrorOut, Fields, Obj, SearchOut};
use emoticond::{Database, Entry, FaceId, Pick, Reason, Report, ReportBuilder, SearchOptions, SearchResult, Target, TermSource, UsageMap, Warning};
use emoticond_config::ConfigError;
use emoticond_state::{PickLogRecord, DISCLAIMER, DISCLAIMER_VERSION};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::time::Instant;

/// This protocol's version (the ready line's `protocol`).
pub const PROTOCOL: u32 = 1;

/// Optional ops and behaviours a client can check for (`features`).
pub const FEATURES: &[&str] = &[
    "opts", "ids", "fields", "explain", "browse", "similar", "complete", "get", "pick", "report", "block", "usage",
    "set_defaults", "supersede", "cancel", "info",
];

/// How many search results `about` can point back to.
const RECENT: usize = 32;

/// Top-level request keys v1 knows; others are ignored with a warning.
const KNOWN_KEYS: &[&str] = &[
    "op", "id", "q", "opts", "fields", "chan", "limit", "explain", "ids", "about", "face", "text", "rank", "reason", "note",
    "reading", "shown", "term_key", "target", "prefix", "map", "texts", "full", "section",
];

/// Ops a `cancel` can drop (nothing that changes state).
const CANCELLABLE: &[&str] = &["search", "browse", "similar", "explain", "complete", "get", "info"];

/// A search the client may point back at with `about`.
struct Recent {
    id: Value,
    q: String,
    result: SearchResult,
}

struct Server {
    emo: Option<Database>,
    app: App,
    /// `set_defaults`: merged under every v1 request's `opts`.
    defaults: Map<String, Value>,
    /// `usage`: a client-supplied usage map, used instead of the store.
    client_usage: Option<UsageMap>,
    recent: VecDeque<Recent>,
    /// The ready line (also the start of `info`).
    ready: String,
}

/// A request read but not started.
struct Item {
    v: Result<Value, String>,
    skip: Option<&'static str>,
}

impl Item {
    fn op(&self) -> Option<&str> {
        self.v.as_ref().ok()?.get("op")?.as_str()
    }
    fn id(&self) -> Option<&Value> {
        self.v.as_ref().ok()?.get("id").filter(|i| !i.is_null())
    }
}

enum Flow {
    Go,
    Quit,
}

fn err_line(id: Option<&Value>, code: &'static str, key: Option<String>, message: impl Into<String>) -> String {
    Obj::new().put("id", id).put("ok", false).put("error", ErrorOut { code, key, message: message.into() }).to_string()
}

fn config_error(e: ConfigError) -> (&'static str, Option<String>, String) {
    match &e {
        ConfigError::InvalidValue { key, .. } | ConfigError::UnknownKey { key } => {
            ("invalid_option", Some(format!("opts.{}", key.trim_start_matches("search."))), e.to_string())
        }
        _ => ("bad_request", None, e.to_string()),
    }
}

/// Mark what a newer queued request makes moot: searches superseded on
/// their channel, and requests a later `cancel` names.
fn mark(queue: &mut VecDeque<Item>) {
    let mut last_on_chan: HashMap<String, usize> = HashMap::new();
    for (i, it) in queue.iter().enumerate() {
        if let (Some("search" | "browse"), Some(c)) = (it.op(), it.v.as_ref().ok().and_then(|v| v.get("chan")).and_then(|c| c.as_str())) {
            last_on_chan.insert(c.to_string(), i);
        }
    }
    let cancels: Vec<(usize, Value)> = queue
        .iter()
        .enumerate()
        .filter(|(_, it)| it.op() == Some("cancel"))
        .filter_map(|(i, it)| Some((i, it.v.as_ref().ok()?.get("target")?.clone())))
        .collect();
    for (i, it) in queue.iter_mut().enumerate() {
        if it.skip.is_some() {
            continue;
        }
        if let (Some("search" | "browse"), Some(c)) = (it.op(), it.v.as_ref().ok().and_then(|v| v.get("chan")).and_then(|c| c.as_str())) {
            if last_on_chan.get(c).is_some_and(|&j| j != i) {
                it.skip = Some("superseded");
                continue;
            }
        }
        if it.op().is_some_and(|op| CANCELLABLE.contains(&op)) {
            if let Some(id) = it.id() {
                if cancels.iter().any(|(j, t)| *j > i && t == id) {
                    it.skip = Some("cancelled");
                }
            }
        }
    }
}

/// Run the protocol on stdin/stdout until EOF, `quit` or the idle timeout.
/// `idle`: seconds (`--idle`), else `daemon.idle_exit`.
pub fn serve(frontend: &str, cfg_args: &ConfigArgs, idle: Option<u64>) -> i32 {
    let cfg = match load_config(frontend, cfg_args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("emoticond: {e}");
            return 2;
        }
    };
    let built = super::build(&cfg);
    let (emo, ms) = (built.db, built.ms);
    let idle_secs = idle.unwrap_or(u64::from(cfg.config.daemon.idle_exit));
    let mut app = App::new(cfg);
    // the overlay layers (user pins, phrases, boosts, hides) at start; bad
    // lines are warnings, on stderr so stdout stays the protocol
    for w in app.load_overlays(emo.as_ref()).into_iter().filter(|w| w.level != emoticond::Level::Info) {
        eprintln!("emoticond: {}", w.message);
    }
    let ready = ready_line(&app, emo.as_ref(), ms);
    #[cfg(feature = "net")]
    start_sender(&app);
    let mut s = Server {
        emo,
        app,
        defaults: Map::new(),
        client_usage: None,
        recent: VecDeque::new(),
        ready,
    };
    let idle = super::idle_exit(idle_secs);
    let out = std::io::stdout();
    let mut out = out.lock();
    // Always announce readiness: stdout is the only channel the client reads.
    let _ = writeln!(out, "{}", s.ready);
    let _ = out.flush();

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut queue: VecDeque<Item> = VecDeque::new();
    let push = |queue: &mut VecDeque<Item>, line: String| {
        let line = line.trim();
        if !line.is_empty() {
            queue.push_back(Item { v: serde_json::from_str(line).map_err(|e| e.to_string()), skip: None });
        }
    };
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(l) => push(&mut queue, l),
                Err(_) => break,
            }
        }
        while let Ok(l) = rx.try_recv() {
            push(&mut queue, l);
        }
        mark(&mut queue);
        let Some(item) = queue.pop_front() else { continue };
        let _busy = idle.as_ref().map(|i| i.busy());
        let flow = match (item.v, item.skip) {
            (Err(e), _) => {
                // Never die on one bad line: a front-end crashing the search
                // backend on a stray byte is far worse than a failed query.
                let _ = writeln!(out, "{}", err_line(None, "bad_json", None, e));
                Flow::Go
            }
            (Ok(v), Some(why)) => {
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                let _ = writeln!(out, "{}", Obj::new().put("id", id).put("ok", false).put(why, true));
                Flow::Go
            }
            (Ok(v), None) => s.handle(&v, &mut out),
        };
        let _ = out.flush();
        if let Flow::Quit = flow {
            break;
        }
    }
    0
}

/// How old a report must be before it is sent: one withdrawn or changed
/// within this time never leaves the machine.
#[cfg(feature = "net")]
const SETTLE_MS: u64 = 120_000;

/// Send settled reports in the background (docs/collector.md): at start,
/// then every minute. Failures leave them queued for the next try; each new
/// kind of failure is logged once.
#[cfg(feature = "net")]
fn start_sender(app: &App) {
    let Some((queue, policy, endpoint)) = app.sender_parts() else { return };
    std::thread::spawn(move || {
        let mut sender = emoticond_state::HttpSender::new(Some(endpoint));
        let mut last_error = String::new();
        loop {
            match queue.flush_settled(&mut sender, policy, now_ms(), SETTLE_MS) {
                Ok(_) => last_error.clear(),
                Err(e) => {
                    let e = e.to_string();
                    if e != last_error {
                        eprintln!("emoticond: reports not sent yet: {e}");
                        last_error = e;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    });
}

/// The ready line (docs/protocol.md): `ms` is how long opening the data took.
fn ready_line(app: &App, emo: Option<&Database>, ms: u128) -> String {
    let faces = emo.map_or(0, |e| e.info().faces);
    let sending = app.sending();
    Obj::new()
        .put("ready", true)
        .put("protocol", PROTOCOL)
        .put("version", emoticond::VERSION)
        .put("data", emo.map(|e| {
            let i = e.info();
            json!({"path": i.path, "version": i.data_version, "dataset": i.dataset, "format": i.format, "faces": i.faces})
        }))
        .put("counts", json!({"emoticon": faces}))
        .put("features", FEATURES)
        .put("disclaimer", json!({"version": DISCLAIMER_VERSION, "short": sending.footer(), "text": DISCLAIMER}))
        .put("sending", sending.on)
        .put("popularity", app.popularity().as_str())
        .put("profile", &app.cfg.profile)
        .put("warnings", app.warnings())
        .put("ms", ms as u64)
        .to_string()
}

/// The acknowledgement of a report (protocol and `emoticond report --json`).
pub fn report_ack(id: Option<&Value>, sending: Sending, s: &Submitted) -> Obj {
    let ids = |v: &[FaceId]| v.iter().map(|i| i.to_string()).collect::<Vec<_>>();
    Obj::new()
        .put_if(id.is_some(), "id", id)
        .put("ok", true)
        .put("queued", true)
        .put("sent", false)
        .put("sending", sending.on)
        .put_if(sending.off_by.is_some(), "sending_off_by", sending.off_by)
        .put("footer", sending.footer())
        .put("effects", &s.effects)
        .put_if(!s.skipped.is_empty(), "skipped", &s.skipped)
        .put_if(!s.blocked.is_empty(), "blocked", ids(&s.blocked))
        .put_if(!s.unblocked.is_empty(), "unblocked", ids(&s.unblocked))
        .put("report_id", &s.report_id)
        .put("key", &s.key)
        .put_if(s.error.is_some(), "warnings", s.error.as_ref().map(|e| [Warning::new("state_io", None, e.clone())]))
}

fn str_list(v: Option<&Value>) -> Vec<&str> {
    v.and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str()).collect()).unwrap_or_default()
}

/// Warnings for top-level keys v1 doesn't know.
fn unknown_keys(v: &Value) -> Vec<Warning> {
    let Value::Object(m) = v else { return Vec::new() };
    m.keys()
        .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
        .map(|k| Warning::new("unknown_key", Some(k), format!("unknown key {k:?} ignored")))
        .collect()
}

impl Server {
    fn handle(&mut self, v: &Value, out: &mut impl Write) -> Flow {
        let id = v.get("id").filter(|i| !i.is_null());
        let Some(op) = v.get("op").and_then(|o| o.as_str()) else {
            let _ = writeln!(out, "{}", err_line(id, "bad_request", Some("op".into()), "\"op\" is required and must be a string"));
            return Flow::Go;
        };
        self.app.refresh(self.emo.as_ref());
        let line = match op {
            "search" | "browse" => Some(self.search(v, id, op == "browse")),
            "similar" => Some(self.similar(v, id)),
            "explain" => Some(self.explain(v, id)),
            "complete" => Some(self.complete(v, id)),
            "get" => Some(self.get(v, id)),
            "pick" => self.pick(v, id),
            "report" => self.report(v, id),
            "block" | "unblock" => self.block(v, id, op == "block"),
            "set_defaults" => Some(self.set_defaults(v, id)),
            "usage" => self.usage(v, id),
            "info" => Some(self.info(id)),
            "cancel" => None,
            "quit" => return Flow::Quit,
            other => Some(err_line(id, "unknown_op", Some("op".into()), format!("unknown op {other:?}"))),
        };
        if let Some(l) = line {
            let _ = writeln!(out, "{l}");
        }
        Flow::Go
    }

    /// The request's search options: `set_defaults`, then the top-level
    /// `limit`/`explain`, then `opts`; resolved through the config
    /// (profile, env, policy); then usage and the blocklist.
    fn options(&self, v: &Value) -> Result<(SearchOptions, Vec<Warning>), String> {
        let id = v.get("id").filter(|i| !i.is_null());
        let mut obj = Value::Object(self.defaults.clone());
        let mut top = Map::new();
        if let Some(l) = v.get("limit") {
            top.insert("limit".into(), l.clone());
        }
        if let Some(e) = v.get("explain") {
            let e = match e {
                Value::Bool(true) => json!("reading"),
                Value::Bool(false) => json!("off"),
                other => other.clone(),
            };
            top.insert("explain".into(), e);
        }
        super::merge(&mut obj, &top);
        match v.get("opts") {
            None | Some(Value::Null) => {}
            Some(Value::Object(o)) => super::merge(&mut obj, o),
            Some(_) => return Err(err_line(id, "invalid_option", Some("opts".into()), "\"opts\" must be an object")),
        }
        let Value::Object(obj) = obj else { unreachable!() };
        let keep_usage = obj.contains_key("usage");
        let (mut o, w) = self.app.options_with(&obj).map_err(|e| {
            let (code, key, msg) = config_error(e);
            err_line(id, code, key, msg)
        })?;
        self.app.finish(&mut o, keep_usage, self.client_usage.as_ref());
        Ok((o, w))
    }

    fn no_data(&self, id: Option<&Value>) -> String {
        err_line(id, "unsupported", None, "no kaomoji data loaded (the emotion engine export is missing)")
    }

    fn remember(&mut self, id: Option<&Value>, q: &str, result: &SearchResult) {
        let Some(id) = id else { return };
        self.recent.retain(|r| &r.id != id);
        self.recent.push_back(Recent { id: id.clone(), q: q.to_string(), result: result.clone() });
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
    }

    fn about(&self, v: &Value) -> Option<&Recent> {
        let a = v.get("about").filter(|a| !a.is_null())?;
        self.recent.iter().rev().find(|r| &r.id == a)
    }

    fn search(&mut self, v: &Value, id: Option<&Value>, browse: bool) -> String {
        let t0 = Instant::now();
        let q = match v.get("q") {
            None | Some(Value::Null) => "",
            Some(Value::String(s)) if !browse => s.as_str(),
            Some(Value::String(_)) => "",
            Some(_) => return err_line(id, "bad_request", Some("q".into()), "\"q\" must be a string"),
        };
        let (mut opts, mut warnings) = match self.options(v) {
            Ok(x) => x,
            Err(line) => return line,
        };
        warnings.splice(0..0, unknown_keys(v));
        match v.get("section").and_then(|s| s.as_str()) {
            Some("recent" | "popular") => opts.pinned = false,
            Some("starter") => opts.usage = UsageMap::default(),
            Some("all") | None => {}
            Some(other) => warnings.push(Warning::new("unknown_section", Some("section"), format!("unknown section {other:?}; showing all"))),
        }
        let fields = Fields::parse(str_list(v.get("fields")), &mut warnings);
        let Some(emo) = self.emo.as_ref() else { return self.no_data(id) };
        let mode = if q.trim().is_empty() { "browse" } else { "search" };
        let r = if mode == "browse" {
            emo.browse(&opts)
        } else {
            emo.search(q, &opts)
        };
        let mut o = SearchOut::of(emo, q, mode, &r, fields, warnings);
        o.id = id.cloned();
        o.ms = (t0.elapsed().as_secs_f64() * 1e4).round() / 10.0;
        self.remember(id, q, &r);
        json(&o)
    }

    /// A face named by id (`k…` or 12 hex) or by its exact text.
    fn resolve(&self, s: &str) -> Option<Entry> {
        let emo = self.emo.as_ref()?;
        match FaceId::parse_any(s) {
            Some(id) => emo.get(id),
            None => emo.find_text(s),
        }
    }

    fn face_arg<'a>(&self, v: &'a Value) -> Option<&'a str> {
        v.get("face").or_else(|| v.get("text")).and_then(|f| f.as_str())
    }

    fn similar(&mut self, v: &Value, id: Option<&Value>) -> String {
        let t0 = Instant::now();
        let Some(face) = self.face_arg(v) else { return err_line(id, "bad_request", Some("face".into()), "similar needs \"face\" (an id or face text)") };
        let (opts, mut warnings) = match self.options(v) {
            Ok(x) => x,
            Err(line) => return line,
        };
        warnings.splice(0..0, unknown_keys(v));
        let fields = Fields::parse(str_list(v.get("fields")), &mut warnings);
        let Some(e) = self.resolve(face) else { return err_line(id, "not_found", Some("face".into()), format!("no face {face:?}")) };
        let Some(emo) = self.emo.as_ref() else { return self.no_data(id) };
        let r = emo.similar(e.id, &opts);
        let mut o = SearchOut::of(emo, &e.text, "similar", &r, fields, warnings);
        o.id = id.cloned();
        o.ms = (t0.elapsed().as_secs_f64() * 1e4).round() / 10.0;
        self.remember(id, &e.text, &r);
        json(&o)
    }

    fn explain(&mut self, v: &Value, id: Option<&Value>) -> String {
        let q = v.get("q").and_then(|q| q.as_str()).unwrap_or("");
        let (mut opts, mut warnings) = match self.options(v) {
            Ok(x) => x,
            Err(line) => return line,
        };
        warnings.splice(0..0, unknown_keys(v));
        let full = v.get("full").and_then(|f| f.as_bool()).unwrap_or(false);
        opts.explain = if full { emoticond::Explain::Full } else { emoticond::Explain::Reading };
        let Some(emo) = self.emo.as_ref() else { return self.no_data(id) };
        let reading = emo.read(q, &opts);
        Obj::new().put("id", id).put("ok", true).put("q", q).put("reading", reading).put_if(!warnings.is_empty(), "warnings", &warnings).to_string()
    }

    fn complete(&mut self, v: &Value, id: Option<&Value>) -> String {
        let prefix = v.get("prefix").or_else(|| v.get("q")).and_then(|p| p.as_str()).unwrap_or("");
        let limit = v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10).clamp(1, 100) as usize;
        let Some(emo) = self.emo.as_ref() else { return self.no_data(id) };
        Obj::new().put("id", id).put("ok", true).put("prefix", prefix).put("completions", emo.complete(prefix, limit)).to_string()
    }

    fn get(&mut self, v: &Value, id: Option<&Value>) -> String {
        let names: Vec<&str> = str_list(v.get("ids")).into_iter().chain(str_list(v.get("texts"))).collect();
        if names.is_empty() {
            return err_line(id, "bad_request", Some("ids".into()), "get needs \"ids\" or \"texts\" (a list)");
        }
        if self.emo.is_none() {
            return self.no_data(id);
        }
        let entries: Vec<Option<EntryOut>> = names.iter().map(|n| self.resolve(n).map(|e| EntryOut::of(&e))).collect();
        Obj::new().put("id", id).put("ok", true).put("entries", entries).to_string()
    }

    /// `{"op":"pick","about":12,"face":"k…","rank":0}`: local popularity
    /// (unless off) and the dev pick log (when on).
    fn pick(&mut self, v: &Value, id: Option<&Value>) -> Option<String> {
        let Some(face) = self.face_arg(v) else {
            return Some(err_line(id, "bad_request", Some("face".into()), "pick needs \"face\" (an id or face text)"));
        };
        let Some(e) = self.resolve(face) else { return Some(err_line(id, "not_found", Some("face".into()), format!("no face {face:?}"))) };
        let (q, term_key, shipped, shown, pos) = match self.about(v) {
            Some(r) => (
                r.q.clone(),
                r.result.term_key.clone(),
                r.result.term_source == TermSource::Shipped,
                r.result.hits.iter().map(|h| h.text.clone()).collect::<Vec<_>>(),
                r.result.hits.iter().position(|h| h.id == e.id),
            ),
            None => {
                let q = v.get("q").and_then(|q| q.as_str()).unwrap_or("").to_string();
                match (q.trim().is_empty(), self.options(v)) {
                    (false, Ok((opts, _))) => {
                        let r = self.emo.as_ref().map(|emo| emo.search(&q, &opts)).unwrap_or_default();
                        let pos = r.hits.iter().position(|h| h.id == e.id);
                        (q, r.term_key.clone(), r.term_source == TermSource::Shipped, r.hits.iter().map(|h| h.text.clone()).collect(), pos)
                    }
                    _ => (q, None, false, Vec::new(), None),
                }
            }
        };
        let rank = v.get("rank").and_then(|r| r.as_u64()).map(|r| r as u32).or(pos.map(|p| p as u32)).unwrap_or(0);
        let mut rec = PickLogRecord::default();
        rec.q = q;
        rec.text = e.text.clone();
        rec.rank = rank;
        rec.shown = shown;
        rec.kind = Some("emoticon".into());
        let (recorded, logged, error) = self.app.pick(&Pick::new(e.id, term_key), shipped, Some(&rec));
        let id = id?;
        let o = Obj::new().put("id", id).put("ok", true).put("recorded", recorded).put("logged", logged);
        let o = match error {
            Some(err) => o.put("warnings", [Warning::new("state_io", None, err)]),
            None => o,
        };
        Some(o.to_string())
    }

    /// A report from either menu (docs/protocol.md "report").
    fn report(&mut self, v: &Value, id: Option<&Value>) -> Option<String> {
        let reason: Reason = match v.get("reason").map(|r| serde_json::from_value::<Reason>(r.clone())) {
            Some(Ok(r)) => r,
            Some(Err(_)) | None => {
                return Some(err_line(
                    id,
                    "bad_request",
                    Some("reason".into()),
                    "reason must be one of read_well, read_wrong, missing, great_fit, other_word, no_fit, offensive, note, clear",
                ))
            }
        };
        let face = match self.face_arg(v) {
            Some(f) => match self.resolve(f) {
                Some(e) => Some(e),
                None => return Some(err_line(id, "not_found", Some("face".into()), format!("no face {f:?}"))),
            },
            None => None,
        };
        if reason.needs_face() && face.is_none() {
            return Some(err_line(id, "bad_request", Some("face".into()), format!("{reason:?} is a face-menu reason: send \"face\"")));
        }
        let builder = match self.report_builder(v, reason, face.as_ref()) {
            Ok(b) => b,
            Err(line) => return Some(line),
        };
        let max = usize::from(self.app.cfg.config.feedback.custom_max_chars);
        let mut builder = builder.max_note_chars(max).stamp(random_id(), now_ms(), DISCLAIMER_VERSION);
        if let Some(n) = v.get("note").and_then(|n| n.as_str()) {
            builder = builder.note(n);
        }
        if let Some(emo) = &self.emo {
            builder = builder.data_version(emo.info().data_version);
        }
        if reason == Reason::Clear {
            // which choice: `clears`, else the newest pending one for this
            // query and target
            let named = v.get("clears").and_then(|c| serde_json::from_value::<Reason>(c.clone()).ok());
            if let Some(c) = named.or_else(|| self.app.last_choice(builder.query(), builder.target())) {
                builder = builder.clears(c);
            }
        }
        let effects = builder.local_effects();
        let report = match builder.build() {
            Ok(r) => r,
            Err(e) => return Some(err_line(id, "bad_request", None, e.to_string())),
        };
        let sending = self.app.sending();
        let line = match self.app.submit(&report, &effects) {
            Ok(s) => report_ack(id, sending, &s).to_string(),
            Err(e) => err_line(id, "internal", None, format!("could not queue the report: {e}")),
        };
        id.map(|_| line)
    }

    /// The report's context: the search `about` points at, or what the
    /// client sends (`q`, `reading`, `term_key`, `shown`), or, failing that,
    /// the query searched again with the request's options.
    fn report_builder(&self, v: &Value, reason: Reason, face: Option<&Entry>) -> Result<ReportBuilder, String> {
        let id = v.get("id").filter(|i| !i.is_null());
        let target = |result: Option<&SearchResult>| match face {
            Some(e) => {
                let rank = v
                    .get("rank")
                    .and_then(|r| r.as_u64())
                    .map(|r| r as u32)
                    .or_else(|| result.and_then(|r| r.hits.iter().find(|h| h.id == e.id)).map(|h| h.rank))
                    .unwrap_or(0);
                Target::Face { id: e.id, text: e.text.clone(), rank }
            }
            None => Target::Query,
        };
        if let Some(r) = self.about(v) {
            let hit = face.and_then(|e| r.result.hits.iter().find(|h| h.id == e.id));
            return Ok(match (face, hit) {
                (Some(_), Some(h)) => Report::for_face(&r.result, &r.q, h, reason),
                (None, _) => Report::for_query(&r.result, &r.q, reason),
                (Some(_), None) => ReportBuilder::new(&r.q, target(Some(&r.result)), reason)
                    .term_key(r.result.term_key.clone())
                    .reading(r.result.reading.as_ref().map(|x| x.line.clone()))
                    .in_force(r.result.safety, r.result.styles)
                    .shown(r.result.hits.iter().map(|h| h.id)),
            });
        }
        let q = v.get("q").and_then(|q| q.as_str()).unwrap_or("");
        let (mut opts, _) = self.options(v)?;
        let mut shown: Vec<FaceId> = str_list(v.get("shown")).into_iter().filter_map(FaceId::parse_any).collect();
        let mut reading = v.get("reading").and_then(|r| r.as_str()).map(String::from);
        let mut term_key = v.get("term_key").and_then(|r| r.as_str()).map(String::from);
        let mut result = None;
        if !q.trim().is_empty() && (shown.is_empty() || reading.is_none() || term_key.is_none()) {
            if opts.explain == emoticond::Explain::Off {
                opts.explain = emoticond::Explain::Reading;
            }
            let r = self.emo.as_ref().map(|e| e.search(q, &opts)).unwrap_or_default();
            if shown.is_empty() {
                shown = r.hits.iter().map(|h| h.id).collect();
            }
            reading = reading.or_else(|| r.reading.as_ref().map(|x| x.line.clone()));
            term_key = term_key.or_else(|| r.term_key.clone());
            result = Some(r);
        } else if q.trim().is_empty() && reason.needs_query() {
            return Err(err_line(id, "bad_request", Some("q".into()), "a query-menu report needs \"about\" or \"q\""));
        }
        Ok(ReportBuilder::new(q, target(result.as_ref()), reason)
            .term_key(term_key)
            .reading(reading)
            .in_force(opts.safety, opts.styles)
            .shown(shown))
    }

    fn block(&mut self, v: &Value, id: Option<&Value>, on: bool) -> Option<String> {
        let Some(face) = self.face_arg(v) else { return Some(err_line(id, "bad_request", Some("face".into()), "send \"face\" (an id or face text)")) };
        let (fid, text) = match self.resolve(face) {
            Some(e) => (e.id, Some(e.text)),
            None => match FaceId::parse_any(face) {
                Some(i) => (i, None),
                None => (FaceId::of_text(face), Some(face.to_string())),
            },
        };
        let r = if on { self.app.block(fid, text.as_deref()) } else { self.app.unblock(fid) };
        let line = match r {
            Ok(changed) => Obj::new().put("id", id).put("ok", true).put("face", fid.to_string()).put("changed", changed).to_string(),
            Err(e) => err_line(id, "internal", None, e.to_string()),
        };
        id.map(|_| line)
    }

    fn set_defaults(&mut self, v: &Value, id: Option<&Value>) -> String {
        let opts = match v.get("opts") {
            Some(Value::Object(o)) => o.clone(),
            None | Some(Value::Null) => Map::new(),
            Some(_) => return err_line(id, "invalid_option", Some("opts".into()), "\"opts\" must be an object"),
        };
        match self.app.options_with(&opts) {
            Ok((_, warnings)) => {
                self.defaults = opts;
                Obj::new().put("id", id).put("ok", true).put_if(!warnings.is_empty(), "warnings", &warnings).to_string()
            }
            Err(e) => {
                let (code, key, msg) = config_error(e);
                err_line(id, code, key, msg)
            }
        }
    }

    fn usage(&mut self, v: &Value, id: Option<&Value>) -> Option<String> {
        let line = match v.get("map") {
            None | Some(Value::Null) => {
                self.client_usage = None;
                Obj::new().put("id", id).put("ok", true).to_string()
            }
            Some(m) => match serde_json::from_value::<UsageMap>(m.clone()) {
                Ok(u) => {
                    self.client_usage = Some(u);
                    Obj::new().put("id", id).put("ok", true).to_string()
                }
                Err(e) => err_line(id, "invalid_option", Some("map".into()), e.to_string()),
            },
        };
        id.map(|_| line)
    }

    fn info(&self, id: Option<&Value>) -> String {
        let mut defaults = self.app.cfg.search.clone();
        defaults.usage = UsageMap::default();
        let p = &self.app.paths;
        let extra = Obj::new()
            .put("id", id)
            .put("ok", true)
            .put("defaults", defaults)
            .put("session_defaults", &self.defaults)
            .put("config_files", &self.app.cfg.files)
            .put("state", json!({"dir": p.root, "usage": p.usage, "blocklist": p.blocklist, "queue": p.queue, "boosts": p.boosts}))
            .put("dev", json!({"pick_log": self.app.pick_log.path()}));
        // the ready line's keys, then these
        format!("{},{}}}", self.ready.strip_suffix('}').unwrap_or(&self.ready), extra.inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(lines: &[&str]) -> VecDeque<Item> {
        lines.iter().map(|l| Item { v: serde_json::from_str(l).map_err(|e: serde_json::Error| e.to_string()), skip: None }).collect()
    }

    #[test]
    fn later_search_on_a_channel_supersedes() {
        let mut q = items(&[
            r#"{"op":"search","id":1,"q":"s","chan":"main"}"#,
            r#"{"op":"search","id":2,"q":"sa","chan":"main"}"#,
            r#"{"op":"search","id":3,"q":"x","chan":"side"}"#,
            r#"{"op":"search","id":4,"q":"sad","chan":"main"}"#,
            r#"{"op":"search","id":5,"q":"no chan"}"#,
            r#"{"id":6,"q":"no op"}"#,
        ]);
        mark(&mut q);
        let skips: Vec<_> = q.iter().map(|i| i.skip).collect();
        assert_eq!(skips, [Some("superseded"), Some("superseded"), None, None, None, None]);
    }

    #[test]
    fn cancel_drops_earlier_read_only_requests() {
        let mut q = items(&[
            r#"{"op":"search","id":1,"q":"s"}"#,
            r#"{"op":"report","id":2,"reason":"note","note":"x","q":"s"}"#,
            r#"{"op":"cancel","target":1}"#,
            r#"{"op":"cancel","target":2}"#,
            r#"{"op":"search","id":3,"q":"s"}"#,
            r#"{"op":"cancel","target":7}"#,
        ]);
        mark(&mut q);
        let skips: Vec<_> = q.iter().map(|i| i.skip).collect();
        assert_eq!(skips, [Some("cancelled"), None, None, None, None, None]);
    }
}
