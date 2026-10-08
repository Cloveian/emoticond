//! emoticond-collector -- receives the reports front-ends send
//! (docs/collector.md) and keeps them in SQLite.
//!
//!     POST /v1/reports   {"reports":[Report, ...]}  ->  {"accepted":[report_id, ...],"rejected":[{"index","error"}]}
//!     POST /v1/stats     StatsUpload (emoticond-state)  ->  {"ok":true}
//!     GET  /healthz      ->  ok
//!
//!     emoticond-collector                 serve ($EMOTICOND_COLLECTOR_ADDR, default 0.0.0.0:8080)
//!     emoticond-collector dump [--since UNIX_MS]   every stored report as JSONL, oldest first
//!     emoticond-collector stats           active installs per day, week and month, and the top faces
//!
//! Usage stats come only from users who said yes. An upload has no id: the
//! collector counts uploads per day, and the client's "first this week /
//! month" flags give weekly and monthly counts. Each upload and each batch
//! in it is stored once (by its random token), so a retry counts once.
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
use serde::{Deserialize, Serialize};
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
         CREATE INDEX IF NOT EXISTS reports_key ON reports (key);
         CREATE TABLE IF NOT EXISTS stats_uploads (
             token       TEXT PRIMARY KEY,
             received_ms INTEGER NOT NULL,
             day         TEXT NOT NULL,
             first_week  INTEGER NOT NULL,
             first_month INTEGER NOT NULL,
             engine      TEXT NOT NULL,
             data        TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS stats_uploads_day ON stats_uploads (day);
         CREATE TABLE IF NOT EXISTS stats_batches (
             token     TEXT PRIMARY KEY,
             upload    TEXT NOT NULL,
             period_from INTEGER NOT NULL,
             period_to   INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS stats_counts (
             batch  TEXT NOT NULL,
             term   TEXT,
             face   TEXT NOT NULL,
             bucket TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS stats_counts_face ON stats_counts (face);",
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

/// A usage-stats upload (emoticond-state's `StatsUpload`).
#[derive(Deserialize)]
struct StatsUpload {
    v: u16,
    token: String,
    engine: String,
    data: String,
    day: String,
    first_this_week: bool,
    first_this_month: bool,
    batches: Vec<StatsBatch>,
}

#[derive(Deserialize)]
struct StatsBatch {
    token: String,
    from: u64,
    to: u64,
    counts: Vec<StatsCount>,
}

#[derive(Deserialize)]
struct StatsCount {
    term: Option<String>,
    id: emoticond::FaceId,
    bucket: String,
}

/// Most batches in one upload, and counts in one batch.
const MAX_STATS_BATCHES: usize = 60;
const MAX_STATS_COUNTS: usize = 10_000;

fn token_ok(t: &str) -> bool {
    (16..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn day_ok(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Store one upload; a repeated token is a retry and stores nothing new.
fn accept_stats(db: &Connection, body: &str) -> Result<(), String> {
    let u: StatsUpload = serde_json::from_str(body).map_err(|e| format!("not a stats upload: {e}"))?;
    if u.v != 1 {
        return Err(format!("unsupported version {}", u.v));
    }
    if !token_ok(&u.token) || !day_ok(&u.day) || u.engine.chars().count() > 32 || u.data.chars().count() > 32 {
        return Err("bad token, day or version field".into());
    }
    if u.batches.len() > MAX_STATS_BATCHES {
        return Err(format!("more than {MAX_STATS_BATCHES} batches"));
    }
    for b in &u.batches {
        if !token_ok(&b.token) || b.counts.len() > MAX_STATS_COUNTS || b.from > b.to {
            return Err("bad batch".into());
        }
        for c in &b.counts {
            if !matches!(c.bucket.as_str(), "1" | "2-4" | "5-19" | "20+") || c.term.as_ref().is_some_and(|t| t.chars().count() > 64) {
                return Err("bad count".into());
            }
        }
    }
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    let new = tx
        .execute(
            "INSERT OR IGNORE INTO stats_uploads (token, received_ms, day, first_week, first_month, engine, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![u.token, now_ms() as i64, u.day, u.first_this_week, u.first_this_month, u.engine, u.data],
        )
        .map_err(|e| e.to_string())?;
    if new > 0 {
        for b in &u.batches {
            let fresh = tx
                .execute(
                    "INSERT OR IGNORE INTO stats_batches (token, upload, period_from, period_to) VALUES (?1, ?2, ?3, ?4)",
                    params![b.token, u.token, b.from as i64, b.to as i64],
                )
                .map_err(|e| e.to_string())?;
            if fresh == 0 {
                continue;
            }
            for c in &b.counts {
                tx.execute(
                    "INSERT INTO stats_counts (batch, term, face, bucket) VALUES (?1, ?2, ?3, ?4)",
                    params![b.token, c.term, c.id.to_string(), c.bucket],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// `emoticond-collector stats`: active installs (that said yes) per day,
/// week and month, and the most picked faces.
fn print_stats(db: &Connection) -> rusqlite::Result<()> {
    let table = |title: &str, sql: &str| -> rusqlite::Result<()> {
        println!("{title}");
        let mut st = db.prepare(sql)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (k, n) = row?;
            println!("  {k}  {n}");
        }
        Ok(())
    };
    table("installs per day (last 14)", "SELECT day, count(*) FROM stats_uploads GROUP BY day ORDER BY day DESC LIMIT 14")?;
    table(
        "installs per week (last 8)",
        "SELECT strftime('%Y-W%W', day), sum(first_week) FROM stats_uploads GROUP BY 1 ORDER BY 1 DESC LIMIT 8",
    )?;
    table("installs per month (last 6)", "SELECT substr(day, 1, 7), sum(first_month) FROM stats_uploads GROUP BY 1 ORDER BY 1 DESC LIMIT 6")?;
    table(
        "most picked faces (picks, roughly)",
        "SELECT face, sum(CASE bucket WHEN '1' THEN 1 WHEN '2-4' THEN 3 WHEN '5-19' THEN 10 ELSE 25 END) AS n FROM stats_counts GROUP BY face ORDER BY n DESC LIMIT 20",
    )?;
    Ok(())
}

fn read_body(req: &mut Request) -> Result<String, Response<std::io::Cursor<Vec<u8>>>> {
    let mut body = Vec::new();
    let read = req.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body);
    match (read, body.len() > MAX_BODY, String::from_utf8(body)) {
        (Err(e), _, _) => Err(error(400, &format!("reading the body: {e}"))),
        (_, true, _) => Err(error(413, "request too large")),
        (_, _, Err(_)) => Err(error(400, "body is not UTF-8")),
        (Ok(_), false, Ok(body)) => Ok(body),
    }
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
                match read_body(&mut req) {
                    Err(r) => r,
                    Ok(body) => match accept(db, &body) {
                        Ok(a) => {
                            limits.reports(&who, a.accepted.len());
                            json_response(200, serde_json::to_string(&a).unwrap_or_default())
                        }
                        Err(e) => error(400, &e),
                    },
                }
            }
        }
        (Method::Post, "/v1/stats") => {
            let who = client(&req);
            if !limits.request(&who) {
                error(429, "too many requests from here; try again later")
            } else {
                match read_body(&mut req) {
                    Err(r) => r,
                    Ok(body) => match accept_stats(db, &body) {
                        Ok(()) => json_response(200, "{\"ok\":true}".into()),
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
        Some("stats") => {
            if let Err(e) = print_stats(&db) {
                eprintln!("emoticond-collector: {e}");
                std::process::exit(1);
            }
        }
        Some("-h" | "--help") => println!("usage: emoticond-collector [dump [--since UNIX_MS] | stats]"),
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
    fn stats_uploads_count_once() {
        let db = open_db(":memory:").unwrap();
        let up = |token: &str, batch: &str| {
            serde_json::json!({
                "v": 1, "token": token, "engine": "1.0.2", "data": "1.0", "day": "2026-10-08",
                "first_this_week": true, "first_this_month": false,
                "batches": [{"v": 1, "token": batch, "from": 1, "to": 2, "engine": "1.0.2", "data": "1.0",
                             "counts": [{"term": "sad", "id": "k6ef56ed18cc6", "bucket": "2-4"}, {"term": null, "id": "k6ef56ed18cc6", "bucket": "1"}]}]
            })
            .to_string()
        };
        let a = "0123456789abcdef0123456789abcdef";
        let b = "fedcba9876543210fedcba9876543210";
        accept_stats(&db, &up(a, b)).unwrap();
        accept_stats(&db, &up(a, b)).unwrap(); // a retry
        let count = |sql: &str| -> i64 { db.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT count(*) FROM stats_uploads"), 1);
        assert_eq!(count("SELECT count(*) FROM stats_counts"), 2);
        assert_eq!(count("SELECT sum(first_week) FROM stats_uploads"), 1);
        assert!(accept_stats(&db, &up("nothex!", b)).is_err());
        assert!(accept_stats(&db, &up(a, b).replace("2-4", "lots")).is_err());
        print_stats(&db).unwrap();
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
