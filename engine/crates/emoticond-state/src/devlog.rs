//! Development logs (docs/options.md §4.4).
//!
//! `[dev] pick_log`: append-only JSONL, **off unless a path is given**,
//! never sent anywhere. One line per pick, keys in sorted order, plus `"ts"`
//! as unix seconds (a float); the eval scripts read `work/eval/picks.jsonl`.

use crate::error::Result;
use crate::fsutil::{append_line, json_line, lock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One pick: `{"kind"?,"q","rank","shown","text","ts"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct PickLogRecord {
    /// The query.
    pub q: String,
    /// The face text chosen.
    pub text: String,
    /// Its rank in the shown list (0-based).
    pub rank: u32,
    /// The faces shown, best first.
    pub shown: Vec<String>,
    /// Old lines say `"emoticon"`; the library is kaomoji only, so new lines
    /// leave it out unless the caller sets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One rating: `{"category","flagged","marks","q","reading","shown","verdict","ts"}`.
/// The picker re-sends the whole record on every click, so the last line
/// for a query is its current state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct RatingLogRecord {
    pub q: String,
    /// `"good"`, `"bad"` or none: how the engine read and ranked the query.
    pub verdict: Option<String>,
    /// "Didn't find what I wanted", whatever the cause.
    pub flagged: bool,
    /// Per-face verdicts by face text: `"good"` / `"bad"`.
    pub marks: BTreeMap<String, String>,
    pub reading: String,
    pub category: String,
    pub shown: Vec<String>,
}

/// An append-only JSONL log, or nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevLog {
    path: Option<PathBuf>,
}

impl DevLog {
    /// A log that writes nothing (the default).
    pub fn off() -> DevLog {
        DevLog { path: None }
    }

    pub fn at(path: impl Into<PathBuf>) -> DevLog {
        DevLog { path: Some(path.into()) }
    }

    /// From a setting or env value (`EMOTICOND_PICK_LOG`): unset, empty or
    /// `"off"` is off.
    pub fn from_setting(v: Option<&str>) -> DevLog {
        match v.map(str::trim) {
            None | Some("") | Some("off") => DevLog::off(),
            Some(p) => DevLog::at(p),
        }
    }

    pub fn is_on(&self) -> bool {
        self.path.is_some()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append a JSON object with `"ts"` (unix seconds, from `now` in ms) set.
    /// Returns false, writing nothing, when the log is off or `record` is
    /// not an object.
    pub fn append(&self, record: &Value, now: u64) -> Result<bool> {
        let Some(path) = &self.path else { return Ok(false) };
        let Value::Object(map) = record else { return Ok(false) };
        let mut map = map.clone();
        map.insert("ts".into(), serde_json::json!(now as f64 / 1000.0));
        let line = json_line(path, &Value::Object(map))?;
        let _g = lock(path)?;
        append_line(path, &line, false)?;
        Ok(true)
    }

    pub fn log_pick(&self, rec: &PickLogRecord, now: u64) -> Result<bool> {
        self.append_typed(rec, now)
    }

    pub fn log_rating(&self, rec: &RatingLogRecord, now: u64) -> Result<bool> {
        self.append_typed(rec, now)
    }

    fn append_typed<T: Serialize>(&self, rec: &T, now: u64) -> Result<bool> {
        let Some(path) = &self.path else { return Ok(false) };
        let v = serde_json::to_value(rec).map_err(|e| crate::Error::Json { path: path.clone(), source: e })?;
        self.append(&v, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn off_writes_nothing() {
        assert!(!DevLog::from_setting(Some("off")).is_on());
        assert!(!DevLog::from_setting(None).is_on());
        assert!(!DevLog::off().append(&serde_json::json!({"q":"x"}), 1).unwrap());
    }

    #[test]
    fn pick_line_matches_the_daemon_shape() {
        let d = TempDir::new();
        let log = DevLog::at(d.path().join("picks.jsonl"));
        let rec = PickLogRecord {
            q: "horrified".into(),
            text: "(⊙_⊙)".into(),
            rank: 3,
            shown: vec!["a".into(), "(⊙_⊙)".into()],
            kind: Some("emoticon".into()),
        };
        assert!(log.log_pick(&rec, 1_791_169_005_643).unwrap());
        let line = std::fs::read_to_string(log.path().unwrap()).unwrap();
        assert_eq!(
            line,
            "{\"kind\":\"emoticon\",\"q\":\"horrified\",\"rank\":3,\"shown\":[\"a\",\"(⊙_⊙)\"],\"text\":\"(⊙_⊙)\",\"ts\":1791169005.643}\n"
        );
        // and it reads back
        let back: PickLogRecord = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn rating_line_and_passthrough() {
        let d = TempDir::new();
        let log = DevLog::at(d.path().join("r.jsonl"));
        let mut rec = RatingLogRecord { q: "shy proud".into(), verdict: Some("good".into()), ..Default::default() };
        rec.marks.insert("(^‿^)".into(), "bad".into());
        log.log_rating(&rec, 2000).unwrap();
        // the daemon's pass-through of whatever the picker sent
        log.append(&serde_json::json!({"q":"x","verdict":null,"extra":1}), 3000).unwrap();
        assert!(!log.append(&serde_json::json!([1]), 3000).unwrap());
        let s = std::fs::read_to_string(log.path().unwrap()).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(
            lines[0],
            "{\"category\":\"\",\"flagged\":false,\"marks\":{\"(^‿^)\":\"bad\"},\"q\":\"shy proud\",\"reading\":\"\",\"shown\":[],\"ts\":2.0,\"verdict\":\"good\"}"
        );
        assert_eq!(lines[1], "{\"extra\":1,\"q\":\"x\",\"ts\":3.0,\"verdict\":null}");
    }
}
