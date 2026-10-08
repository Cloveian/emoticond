//! emoticond-collector -- receives the reports front-ends send
//! (docs/collector.md) and keeps them in SQLite.
//!
//!     POST /v1/reports   {"reports":[Report, ...]}  ->  {"accepted":[report_id, ...],"rejected":[{"index","error"}]}
//!     GET  /healthz      ->  ok
//!
//!     emoticond-collector                 serve ($EMOTICOND_COLLECTOR_ADDR, default 0.0.0.0:8080)
//!     emoticond-collector dump [--since UNIX_MS]   every stored report as JSONL, oldest first
//!
//! The database is $EMOTICOND_COLLECTOR_DB (default /data/reports.db). A
//! report is accepted when it parses as the library's `Report` and passes
//! the size checks; storing is idempotent on `report_id`, so a client that
//! resends after a lost answer gets the same ids back and marks them sent.
//! Nothing about the sender is stored: no IP, no headers. The IP (from
//! `CF-Connecting-IP`, else `X-Forwarded-For`, else the peer) is only used,
//! in memory, for the rate limit.

use emoticond::report::{NOTE_MAX_CHARS, REPORT_VERSION, SHOWN_MAX};
use emoticond::Report;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

/// Largest request body read.
const MAX_BODY: usize = 512 * 1024;
/// Most reports in one request (the client sends batches of 50).
const MAX_BATCH: usize = 50;
/// Longest query, reading or face text kept, in chars.
const MAX_TEXT: usize = 500;
/// Rate limit per client: this many reports per window, and requests.
const RATE_REPORTS: u32 = 300;
const RATE_REQUESTS: u32 = 120;
const RATE_WINDOW: Duration = Duration::from_secs(3600);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn open_db(path: &str) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS reports (
             report_id   TEXT PRIMARY KEY,
             received_ms INTEGER NOT NULL,
             ts          INTEGER NOT NULL,
             key         TEXT NOT NULL,
             reason      TEXT NOT NULL,
             query       TEXT NOT NULL,
             face        TEXT,
             data_version TEXT,
             json        TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS reports_received ON reports (received_ms);
         CREATE INDEX IF NOT EXISTS reports_key ON reports (key);",
    )?;
    Ok(db)
}

#[derive(Serialize)]
struct Rejected {
    index: usize,
    error: String,
}

#[derive(Serialize)]
struct Answer {
    accepted: Vec<String>,
    rejected: Vec<Rejected>,
}

/// Why a parsed report is refused, if it is.
fn check(r: &Report) -> Option<String> {
    let long = |s: &str| s.chars().count() > MAX_TEXT;
    if r.v != REPORT_VERSION {
        return Some(format!("unsupported report version {}", r.v));
    }
    if r.report_id.is_empty() || r.report_id.len() > 64 || r.key.len() > 64 {
        return Some("bad report_id or key".into());
    }
    if r.shown.len() > SHOWN_MAX {
        return Some(format!("shown has more than {SHOWN_MAX} faces"));
    }
    if r.note.as_deref().is_some_and(|n| n.chars().count() > NOTE_MAX_CHARS.max(2000)) {
        return Some("note too long".into());
    }
    if long(&r.query) || r.reading.as_deref().is_some_and(long) || r.corrected.as_deref().is_some_and(long) {
        return Some("query or reading too long".into());
    }
    if let emoticond::Target::Face { text, .. } = &r.target {
        if long(text) {
            return Some("face text too long".into());
        }
    }
    None
}

/// Parse and store one request body.
fn accept(db: &Connection, body: &str) -> Result<Answer, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let list = v.get("reports").and_then(|r| r.as_array()).ok_or("expected {\"reports\":[...]}")?;
    if list.len() > MAX_BATCH {
        return Err(format!("at most {MAX_BATCH} reports per request"));
    }
    let mut answer = Answer { accepted: Vec::new(), rejected: Vec::new() };
    let received = now_ms();
    for (index, raw) in list.iter().enumerate() {
        let report: Report = match serde_json::from_value(raw.clone()) {
            Ok(r) => r,
            Err(e) => {
                answer.rejected.push(Rejected { index, error: e.to_string() });
                continue;
            }
        };
        if let Some(error) = check(&report) {
            answer.rejected.push(Rejected { index, error });
            continue;
        }
        let face = match &report.target {
            emoticond::Target::Face { text, .. } => Some(text.as_str()),
            _ => None,
        };
        let reason = serde_json::to_value(report.reason).ok().and_then(|r| r.as_str().map(String::from)).unwrap_or_default();
        let json = serde_json::to_string(&report).map_err(|e| e.to_string())?;
        let res = db.execute(
            "INSERT OR IGNORE INTO reports (report_id, received_ms, ts, key, reason, query, face, data_version, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                report.report_id,
                received as i64,
                report.ts as i64,
                report.key,
                reason,
                report.query,
                face,
                report.engine.data,
                json
            ],
        );
        match res {
            Ok(_) => answer.accepted.push(report.report_id),
            Err(e) => {
                eprintln!("emoticond-collector: storing a report: {e}");
                answer.rejected.push(Rejected { index, error: "could not store".into() });
            }
        }
    }
    Ok(answer)
}

/// Requests and reports per client in the current window.
struct Limits {
    window_start: Instant,
    seen: HashMap<String, (u32, u32)>,
}

impl Limits {
    fn new() -> Limits {
        Limits { window_start: Instant::now(), seen: HashMap::new() }
    }

    /// Count a request from `who`; false when it is over the limit.
    fn request(&mut self, who: &str) -> bool {
        if self.window_start.elapsed() >= RATE_WINDOW {
            self.window_start = Instant::now();
            self.seen.clear();
        }
        let e = self.seen.entry(who.to_string()).or_default();
        e.0 += 1;
        e.0 <= RATE_REQUESTS && e.1 < RATE_REPORTS
    }

    fn reports(&mut self, who: &str, n: usize) {
        self.seen.entry(who.to_string()).or_default().1 += n as u32;
    }
}

fn client(req: &Request) -> String {
    let header = |name: &'static str| {
        req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().split(',').next().unwrap_or("").trim().to_string())
    };
    header("CF-Connecting-IP")
        .filter(|s| !s.is_empty())
        .or_else(|| header("X-Forwarded-For").filter(|s| !s.is_empty()))
        .or_else(|| req.remote_addr().map(|a| a.ip().to_string()))
        .unwrap_or_default()
}

fn json_response(status: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let ct = Header::from_bytes("Content-Type", "application/json").expect("static header");
    Response::from_string(body).with_status_code(status).with_header(ct)
}

fn error(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(status, serde_json::json!({ "error": msg }).to_string())
}

fn handle(db: &Connection, limits: &mut Limits, mut req: Request) {
    let path = req.url().split('?').next().unwrap_or("").to_string();
    let resp = match (req.method(), path.as_str()) {
        (Method::Get, "/healthz") => Response::from_string("ok"),
        (Method::Post, "/v1/reports") => {
            let who = client(&req);
            if !limits.request(&who) {
                error(429, "too many reports from here; try again later")
            } else {
                let mut body = Vec::new();
                let read = req.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body);
                match (read, body.len() > MAX_BODY, String::from_utf8(body)) {
                    (Err(e), _, _) => error(400, &format!("reading the body: {e}")),
                    (_, true, _) => error(413, "request too large"),
                    (_, _, Err(_)) => error(400, "body is not UTF-8"),
                    (Ok(_), false, Ok(body)) => match accept(db, &body) {
                        Ok(a) => {
                            limits.reports(&who, a.accepted.len());
                            json_response(200, serde_json::to_string(&a).unwrap_or_default())
                        }
                        Err(e) => error(400, &e),
                    },
                }
            }
        }
        _ => error(404, "not found"),
    };
    let _ = req.respond(resp);
}

fn dump(db: &Connection, since: i64) -> rusqlite::Result<()> {
    let mut st = db.prepare("SELECT json, received_ms FROM reports WHERE received_ms >= ?1 ORDER BY received_ms, report_id")?;
    let rows = st.query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (json, received) = row?;
        match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(mut v) => {
                v["received_ms"] = received.into();
                println!("{v}");
            }
            Err(_) => println!("{json}"),
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = std::env::var("EMOTICOND_COLLECTOR_DB").unwrap_or_else(|_| "/data/reports.db".into());
    let db = match open_db(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("emoticond-collector: opening {db_path}: {e}");
            std::process::exit(1);
        }
    };
    match args.get(1).map(String::as_str) {
        Some("dump") => {
            let since = args.iter().position(|a| a == "--since").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Err(e) = dump(&db, since) {
                eprintln!("emoticond-collector: {e}");
                std::process::exit(1);
            }
        }
        Some("-h" | "--help") => println!("usage: emoticond-collector [dump [--since UNIX_MS]]"),
        Some(other) => {
            eprintln!("emoticond-collector: unknown command {other:?}");
            std::process::exit(2);
        }
        None => {
            let addr = std::env::var("EMOTICOND_COLLECTOR_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
            let server = match Server::http(&addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("emoticond-collector: listening on {addr}: {e}");
                    std::process::exit(1);
                }
            };
            eprintln!("emoticond-collector: listening on {addr}, storing in {db_path}");
            let mut limits = Limits::new();
            for req in server.incoming_requests() {
                handle(&db, &mut limits, req);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emoticond::{Reason, ReportBuilder, Target};

    fn report(id: &str) -> Report {
        ReportBuilder::new("thats a lot", Target::Query, Reason::Missing).stamp(id, 1_791_000_000_000, 1).build().unwrap()
    }

    #[test]
    fn stores_valid_reports_idempotently_and_rejects_bad_ones() {
        let db = open_db(":memory:").unwrap();
        let good = serde_json::to_value(report("r1")).unwrap();
        let body = serde_json::json!({"reports": [good, {"nope": 1}]}).to_string();
        let a = accept(&db, &body).unwrap();
        assert_eq!(a.accepted, ["r1"]);
        assert_eq!(a.rejected.len(), 1);
        assert_eq!(a.rejected[0].index, 1);
        // a resend is accepted again (so the client marks it sent) but stored once
        let a = accept(&db, &body).unwrap();
        assert_eq!(a.accepted, ["r1"]);
        let n: i64 = db.query_row("SELECT count(*) FROM reports", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert!(accept(&db, "[]").is_err());
        let many = serde_json::json!({"reports": vec![serde_json::Value::Null; MAX_BATCH + 1]}).to_string();
        assert!(accept(&db, &many).is_err());
    }

    #[test]
    fn rate_limit_counts_requests_and_reports() {
        let mut l = Limits::new();
        for _ in 0..RATE_REQUESTS {
            assert!(l.request("a"));
        }
        assert!(!l.request("a"));
        assert!(l.request("b"));
        l.reports("b", RATE_REPORTS as usize);
        assert!(!l.request("b"));
    }
}
