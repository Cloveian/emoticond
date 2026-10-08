//! The report queue and sending (docs/options.md §5,
//! docs/collector.md).
//!
//! Clicking a report item always creates a report: it is appended to
//! `reports/queue.jsonl` and its local effects apply at once. Sending is a
//! separate step through a [`Sender`], and is refused unless the
//! [`SendPolicy`] allows it. Two parties can switch sending off:
//! - the **user** (`feedback.send = false`);
//! - the **packager**: `Policy::reports_disabled` (`/etc/emoticond/policy.toml`,
//!   read by `emoticond-config`), or building without the `net` feature, in
//!   which case this crate contains no network code at all.
//!
//! Supersession: reports with the same `key` (same query and target)
//! replace each other, last line wins. A `Clear` report withdraws the
//! earlier one: if that was never sent, nothing is pending for the key; if
//! it was sent, the `Clear` itself is pending so the collector learns of it.

use crate::effects::{Applied, LocalEffects};
use crate::error::{Error, Result};
use crate::fsutil::{append_line, json_line, lock, read_optional, write_atomic};
use crate::paths::StatePaths;
use emoticond::{LocalEffect, Policy, Reason, Report, Warning};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The version of [`DISCLAIMER`]. Reports record the version shown
/// (`ReportBuilder::stamp`); raise it whenever what a report contains
/// changes, so front-ends show the full text again (options.md §5.4).
pub const DISCLAIMER_VERSION: u32 = 2;

/// The full disclaimer, shown the first time a report menu opens and again
/// whenever [`DISCLAIMER_VERSION`] goes up.
pub const DISCLAIMER: &str = "Reporting helps improve emoticond search. When you click a report item, \
a report is saved on this computer and, if you chose to send reports, sent to the emoticond project. \
A report contains: the text you searched for, how the search was read (the reading line), \
the menu item you chose, the face you reported and its place in the list, the top 20 faces shown, \
any note you write, your safety and style settings, the library and data versions, the time, \
and a random report id. It does not contain your usage history, other searches, an install id, \
your locale, time zone or computer name. Reporting a face as offensive also hides it on this \
computer at once, whether or not the report is sent. Nothing is sent until you say yes to sending \
reports, only reports made after that are sent, you can change your answer any time \
(`emoticond reports on|off`), and your system administrator may have turned sending off.";

/// The one-line footer for a report menu.
pub fn disclaimer_footer(send: SendPolicy) -> &'static str {
    match send {
        SendPolicy::Allowed => "Reports are sent with your search, its reading and the top 20 faces shown.",
        SendPolicy::Off(SendOff::Unasked) => "Reports are saved on this computer until you choose whether to send them.",
        SendPolicy::Off(_) => "Reports are saved on this computer only.",
    }
}

/// Why sending is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendOff {
    /// The user's `feedback.send = false`, or their answer "no".
    User,
    /// The user hasn't been asked yet: nothing is sent until they say yes.
    Unasked,
    /// The packager's policy (`Policy::reports_disabled`).
    Policy,
    /// Built without the `net` feature: no network code exists.
    NotBuilt,
}

impl std::fmt::Display for SendOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SendOff::User => "turned off in settings",
            SendOff::Unasked => "not chosen yet (emoticond reports on)",
            SendOff::Policy => "turned off by your system administrator",
            SendOff::NotBuilt => "this build has no network support",
        })
    }
}

/// Whether reports may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendPolicy {
    Allowed,
    Off(SendOff),
}

impl SendPolicy {
    /// Combine the policy, this build and the user's `feedback.send`. The
    /// packager's switches are checked first, so a locked setting reports
    /// [`SendOff::Policy`] (the UI says "set by your system administrator").
    ///
    /// Without the `net` feature this is always `Off(NotBuilt)`. A front-end
    /// with a delivery path of its own may still pass `SendPolicy::Allowed`
    /// to [`FeedbackQueue::flush`], but must honour the policy and user
    /// setting itself.
    pub fn new(policy: &Policy, user_send: bool) -> SendPolicy {
        if policy.reports_disabled {
            SendPolicy::Off(SendOff::Policy)
        } else if !cfg!(feature = "net") {
            SendPolicy::Off(SendOff::NotBuilt)
        } else if !user_send {
            SendPolicy::Off(SendOff::User)
        } else {
            SendPolicy::Allowed
        }
    }

    pub fn allows(self) -> bool {
        self == SendPolicy::Allowed
    }
}

/// Why a [`Sender`] failed. The reports stay queued.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SendError {
    #[error("no report endpoint is configured")]
    NoEndpoint,
    #[error("the collector rejected the reports: {0}")]
    Rejected(String),
    #[error("could not reach the collector: {0}")]
    Transport(String),
    #[error("{0}")]
    Unsupported(&'static str),
}

/// Delivers reports. Returns the `report_id`s the collector accepted;
/// those are marked sent, the rest stay queued.
pub trait Sender {
    fn send(&mut self, batch: &[Report]) -> std::result::Result<Vec<String>, SendError>;
}

/// The HTTP sender (`net` feature): POSTs `{"reports":[...]}` to the
/// collector (`feedback.endpoint`, docs/collector.md) and returns the ids
/// in its `accepted` list.
#[cfg(feature = "net")]
#[derive(Debug, Clone, Default)]
pub struct HttpSender {
    /// `feedback.endpoint` (policy may override). None: nowhere to send.
    pub endpoint: Option<String>,
    /// Per request; default 10 s.
    pub timeout: Option<std::time::Duration>,
}

#[cfg(feature = "net")]
impl HttpSender {
    pub fn new(endpoint: Option<String>) -> HttpSender {
        HttpSender { endpoint, timeout: None }
    }
}

#[cfg(feature = "net")]
impl Sender for HttpSender {
    fn send(&mut self, batch: &[Report]) -> std::result::Result<Vec<String>, SendError> {
        let Some(url) = self.endpoint.as_deref().filter(|u| !u.trim().is_empty()) else { return Err(SendError::NoEndpoint) };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout.unwrap_or(std::time::Duration::from_secs(10))))
            .user_agent(concat!("emoticond/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let mut resp = match agent.post(url).send_json(serde_json::json!({ "reports": batch })) {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(code)) => return Err(SendError::Rejected(format!("HTTP {code}"))),
            Err(e) => return Err(SendError::Transport(e.to_string())),
        };
        let v: serde_json::Value = resp.body_mut().read_json().map_err(|e| SendError::Rejected(format!("bad answer: {e}")))?;
        let accepted = v.get("accepted").and_then(|a| a.as_array()).ok_or_else(|| SendError::Rejected("answer has no \"accepted\" list".into()))?;
        Ok(accepted.iter().filter_map(|x| x.as_str().map(String::from)).collect())
    }
}

/// `feedback.queue_max` and `feedback.queue_max_age_days`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueueLimits {
    /// Pending reports kept; the oldest are dropped beyond this (500).
    pub queue_max: usize,
    /// Pending reports older than this are dropped (180).
    pub max_age_days: u16,
}

impl Default for QueueLimits {
    fn default() -> QueueLimits {
        QueueLimits { queue_max: 500, max_age_days: 180 }
    }
}

/// One line of `reports/sent/sent.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SentRecord {
    report_id: String,
    key: String,
    /// The sent report was a `Clear`.
    #[serde(default)]
    clear: bool,
    sent_at: u64,
}

/// The reports waiting to be sent, oldest first, after supersession and the
/// queue limits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pending {
    pub reports: Vec<Report>,
    /// Queue lines that did not parse (they are dropped at compaction).
    pub warnings: Vec<Warning>,
}

impl Pending {
    pub fn iter(&self) -> std::slice::Iter<'_, Report> {
        self.reports.iter()
    }
    pub fn len(&self) -> usize {
        self.reports.len()
    }
    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }
}

impl IntoIterator for Pending {
    type Item = Report;
    type IntoIter = std::vec::IntoIter<Report>;
    fn into_iter(self) -> Self::IntoIter {
        self.reports.into_iter()
    }
}

impl<'a> IntoIterator for &'a Pending {
    type Item = &'a Report;
    type IntoIter = std::slice::Iter<'a, Report>;
    fn into_iter(self) -> Self::IntoIter {
        self.reports.iter()
    }
}

/// What [`FeedbackQueue::flush`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlushOutcome {
    pub sent: usize,
    pub remaining: usize,
}

/// Reports per [`Sender::send`] call.
const BATCH: usize = 50;

/// `reports/queue.jsonl` and the sent log.
#[derive(Debug, Clone)]
pub struct FeedbackQueue {
    queue: PathBuf,
    sent_log: PathBuf,
    limits: QueueLimits,
}

impl FeedbackQueue {
    pub fn new(paths: &StatePaths, limits: QueueLimits) -> FeedbackQueue {
        FeedbackQueue::at(paths.queue.clone(), paths.sent_log.clone(), limits)
    }

    pub fn at(queue: impl Into<PathBuf>, sent_log: impl Into<PathBuf>, limits: QueueLimits) -> FeedbackQueue {
        FeedbackQueue { queue: queue.into(), sent_log: sent_log.into(), limits }
    }

    pub fn path(&self) -> &Path {
        &self.queue
    }

    /// What a click on a report item does: apply the local effects (the
    /// offensive hide first, and always), then queue the report. The report
    /// is queued even if an effect failed; the first effect error is then
    /// returned. Sending is separate ([`flush`](Self::flush)).
    pub fn submit(
        &self,
        report: &Report,
        effects: &[LocalEffect],
        local: &mut LocalEffects<'_>,
        now: u64,
    ) -> Result<Applied> {
        let applied = local.apply(effects, now);
        self.append(report, now)?;
        applied
    }

    /// Append `report` to the queue (one line, under the lock). Compacts the
    /// file when it has grown well past `queue_max` lines.
    pub fn append(&self, report: &Report, now: u64) -> Result<()> {
        let _g = lock(&self.queue)?;
        append_line(&self.queue, &json_line(&self.queue, report)?, true)?;
        let lines = read_optional(&self.queue)?.map_or(0, |b| b.iter().filter(|c| **c == b'\n').count());
        if lines > (self.limits.queue_max.max(32)) * 2 {
            self.compact_locked(now)?;
        }
        Ok(())
    }

    fn read_queue(&self) -> Result<(Vec<Report>, Vec<Warning>)> {
        let mut reports = Vec::new();
        let mut warnings = Vec::new();
        let Some(b) = read_optional(&self.queue)? else { return Ok((reports, warnings)) };
        for (n, line) in String::from_utf8_lossy(&b).lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Report>(line) {
                Ok(r) => reports.push(r),
                Err(e) => warnings.push(Warning::new(
                    "queue_bad_line",
                    None,
                    format!("{}:{}: {e}; line ignored", self.queue.display(), n + 1),
                )),
            }
        }
        Ok((reports, warnings))
    }

    fn read_sent(&self) -> Result<Vec<SentRecord>> {
        let Some(b) = read_optional(&self.sent_log)? else { return Ok(Vec::new()) };
        Ok(String::from_utf8_lossy(&b).lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
    }

    /// The pending set at `now`: the last report per key, minus those sent,
    /// minus `Clear`s with nothing sent to withdraw, minus reports older than
    /// `max_age_days`, keeping at most the newest `queue_max`.
    pub fn pending(&self, now: u64) -> Result<Pending> {
        let (reports, warnings) = self.read_queue()?;
        let sent = self.read_sent()?;
        let sent_ids: HashSet<&str> = sent.iter().map(|s| s.report_id.as_str()).collect();
        // per key: was the last thing sent an active report (not a Clear)?
        let mut live_sent: HashMap<&str, bool> = HashMap::new();
        for s in &sent {
            live_sent.insert(s.key.as_str(), !s.clear);
        }
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (i, r) in reports.iter().enumerate() {
            last.insert(r.key.as_str(), i);
        }
        let max_age = u64::from(self.limits.max_age_days) * 86_400_000;
        let mut keep: Vec<usize> = last
            .into_iter()
            .filter(|(key, i)| {
                let r = &reports[*i];
                if sent_ids.contains(r.report_id.as_str()) {
                    return false;
                }
                if r.reason == Reason::Clear && !live_sent.get(key).copied().unwrap_or(false) {
                    return false;
                }
                r.ts.saturating_add(max_age) >= now
            })
            .map(|(_, i)| i)
            .collect();
        keep.sort_unstable();
        let drop = keep.len().saturating_sub(self.limits.queue_max);
        let keep: HashSet<usize> = keep.into_iter().skip(drop).collect();
        let reports = reports.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, r)| r).collect();
        Ok(Pending { reports, warnings })
    }

    /// Record that `reports` were delivered.
    pub fn mark_sent<'r>(&self, reports: impl IntoIterator<Item = &'r Report>, now: u64) -> Result<()> {
        let _g = lock(&self.sent_log)?;
        for r in reports {
            let rec = SentRecord {
                report_id: r.report_id.clone(),
                key: r.key.clone(),
                clear: r.reason == Reason::Clear,
                sent_at: now,
            };
            append_line(&self.sent_log, &json_line(&self.sent_log, &rec)?, true)?;
        }
        Ok(())
    }

    /// Rewrite the queue to hold only the pending set (atomically).
    pub fn compact(&self, now: u64) -> Result<()> {
        let _g = lock(&self.queue)?;
        self.compact_locked(now)
    }

    fn compact_locked(&self, now: u64) -> Result<()> {
        let pending = self.pending(now)?;
        let mut out = String::new();
        for r in &pending {
            out.push_str(&json_line(&self.queue, r)?);
            out.push('\n');
        }
        write_atomic(&self.queue, out.as_bytes())
    }

    /// Send the pending reports through `sender`, in batches, marking the
    /// accepted ones sent. Refused with [`Error::SendingOff`] unless `policy`
    /// allows sending; on a sender error the rest stay queued.
    pub fn flush(&self, sender: &mut dyn Sender, policy: SendPolicy, now: u64) -> Result<FlushOutcome> {
        self.flush_settled(sender, policy, now, 0, 0)
    }

    /// [`flush`](Self::flush), but only reports at least `settle_ms` old: one
    /// withdrawn or changed within that time is never sent. The younger
    /// ones count as `remaining`.
    ///
    /// Reports made before `since_ms` are never sent (the user said yes
    /// after making them); 0 sends them all.
    pub fn flush_settled(&self, sender: &mut dyn Sender, policy: SendPolicy, now: u64, settle_ms: u64, since_ms: u64) -> Result<FlushOutcome> {
        if let SendPolicy::Off(why) = policy {
            return Err(Error::SendingOff(why));
        }
        let pending = self.pending(now)?;
        let total = pending.len();
        let ready: Vec<Report> = pending.reports.into_iter().filter(|r| r.ts >= since_ms && r.ts.saturating_add(settle_ms) <= now).collect();
        let mut sent = 0;
        for chunk in ready.chunks(BATCH) {
            let accepted: HashSet<String> = sender.send(chunk)?.into_iter().collect();
            let done: Vec<&Report> = chunk.iter().filter(|r| accepted.contains(&r.report_id)).collect();
            self.mark_sent(done.iter().copied(), now)?;
            sent += done.len();
        }
        Ok(FlushOutcome { sent, remaining: total - sent })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_policy_layers() {
        let mut locked = Policy::default();
        locked.reports_disabled = true;
        assert_eq!(SendPolicy::new(&locked, true), SendPolicy::Off(SendOff::Policy));
        if cfg!(feature = "net") {
            assert_eq!(SendPolicy::new(&Policy::default(), true), SendPolicy::Allowed);
            assert_eq!(SendPolicy::new(&Policy::default(), false), SendPolicy::Off(SendOff::User));
        } else {
            assert_eq!(SendPolicy::new(&Policy::default(), true), SendPolicy::Off(SendOff::NotBuilt));
        }
        assert!(Error::SendingOff(SendOff::Policy).is_sending_locked());
        assert!(!Error::SendingOff(SendOff::User).is_sending_locked());
    }

    #[test]
    fn disclaimer_mentions_what_decisions_7_adds() {
        assert!(DISCLAIMER.contains("top 20"));
        assert!(DISCLAIMER.contains("reading"));
        assert_eq!(disclaimer_footer(SendPolicy::Off(SendOff::User)), "Reports are saved on this computer only.");
    }

    #[cfg(feature = "net")]
    #[test]
    fn http_sender_posts_and_reads_accepted_ids() {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut s = HttpSender::new(None);
        assert_eq!(s.send(&[]), Err(SendError::NoEndpoint));
        // nothing listening: a transport error, reports stay queued
        let mut s = HttpSender::new(Some("http://127.0.0.1:9/v1/reports".into()));
        assert!(matches!(s.send(&[]), Err(SendError::Transport(_))));
        // a one-shot collector that accepts "r1"
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/reports", l.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (c, _) = l.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let answer = r#"{"accepted":["r1"],"rejected":[]}"#;
            write!(&c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).unwrap();
            String::from_utf8(body).unwrap()
        });
        let mut s = HttpSender::new(Some(url));
        assert_eq!(s.send(&[]).unwrap(), ["r1"]);
        let body: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({"reports": []}));
    }
}
