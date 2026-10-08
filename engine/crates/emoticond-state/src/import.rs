//! One-off import of a dev pick log (`dev.pick_log`) into a
//! [`UsageState`].
//!
//! Each pick's face text is hashed to its [`FaceId`] and its query turned
//! into a concept by the caller's `term_of` (ideally the engine's
//! `term_key` for that query; [`normalize_query`] is a fallback). The raw
//! text and query are not kept. Picks are replayed oldest first at their own
//! times, so the result is what recording them live would have produced.
//!
//! To fold the result into `usage.json`, run it inside
//! [`UsageStore::update`](crate::UsageStore::update); see
//! `examples/import_picks.rs`.

use crate::error::{Error, Result};
use emoticond::usage as ku;
use emoticond::{FaceId, Pick, UsageState, Warning};
use serde_json::Value;
use std::path::Path;

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ImportSummary {
    /// Picks recorded.
    pub imported: usize,
    /// Lines skipped: not kaomoji (old emoji/symbol picks), no text, or bad
    /// JSON (the last also gives a warning).
    pub skipped: usize,
    pub warnings: Vec<Warning>,
}

/// Lowercase, trim and collapse whitespace; `None` for an empty query.
pub fn normalize_query(q: &str) -> Option<String> {
    let s = q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    (!s.is_empty()).then_some(s)
}

/// Replay the picks in the JSONL file at `path` into `state`.
pub fn import_picks(
    path: &Path,
    state: &mut UsageState,
    mut term_of: impl FnMut(&str) -> Option<String>,
) -> Result<ImportSummary> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut sum = ImportSummary::default();
    let mut picks: Vec<(u64, FaceId, Option<String>)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                sum.skipped += 1;
                sum.warnings.push(Warning::new(
                    "import_bad_line",
                    None,
                    format!("{}:{}: {e}; line skipped", path.display(), n + 1),
                ));
                continue;
            }
        };
        let kind_ok = v.get("kind").and_then(Value::as_str).is_none_or(|k| k == "emoticon");
        let face = v.get("text").and_then(Value::as_str).filter(|t| !t.is_empty());
        let (true, Some(face)) = (kind_ok, face) else {
            sum.skipped += 1;
            continue;
        };
        let ts = v.get("ts").and_then(Value::as_f64).map_or(0, |s| (s * 1000.0).max(0.0) as u64);
        let term = v.get("q").and_then(Value::as_str).and_then(&mut term_of);
        picks.push((ts, FaceId::of_text(face), term));
    }
    picks.sort_by_key(|p| p.0);
    for (ts, id, term) in picks {
        ku::record(state, &Pick::new(id, term), ts);
        sum.imported += 1;
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn normalizes() {
        assert_eq!(normalize_query("  IDK   lol "), Some("idk lol".into()));
        assert_eq!(normalize_query("   "), None);
    }

    #[test]
    fn imports_kaomoji_picks_in_time_order() {
        let d = TempDir::new();
        let p = d.path().join("picks.jsonl");
        std::fs::write(
            &p,
            concat!(
                "{\"kind\":\"emoticon\",\"q\":\"Shrug\",\"rank\":0,\"shown\":[],\"text\":\"¯\\\\_(ツ)_/¯\",\"ts\":2000.0}\n",
                "{\"kind\":\"emoji\",\"q\":\"cat\",\"text\":\"🐱\",\"ts\":1000.0}\n",
                "not json\n",
                "{\"q\":\"shrug\",\"text\":\"¯\\\\_(ツ)_/¯\",\"ts\":1000.0}\n",
                "{\"q\":\"\",\"text\":\"(^‿^)\",\"ts\":1500.5}\n",
            ),
        )
        .unwrap();
        let mut st = UsageState::default();
        let s = import_picks(&p, &mut st, normalize_query).unwrap();
        assert_eq!((s.imported, s.skipped, s.warnings.len()), (3, 2, 1));
        let shrug = FaceId::of_text("¯\\_(ツ)_/¯");
        assert_eq!(st.updated, 2_000_000);
        assert!(st.global[&shrug] > 1.99 && st.global[&shrug] <= 2.0);
        assert!(st.by_term["shrug"][&shrug] > 1.99);
        assert_eq!(st.by_term.len(), 1);
        assert!(st.global.contains_key(&FaceId::of_text("(^‿^)")));
        // no raw text kept
        assert!(!serde_json::to_string(&st).unwrap().contains("ツ"));
    }
}
