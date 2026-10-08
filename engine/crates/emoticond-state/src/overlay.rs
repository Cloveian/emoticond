//! Machine-written overlays (docs/options.md §6.1
//! §2.5, §4.1).
//!
//! "Doesn't fit" reports write a negative boost to
//! `$XDG_STATE_HOME/emoticond/overlays/boosts.jsonl`, in the same format as the
//! shipped `data/boosts.jsonl` (`{"term","id","boost","why"}`, id as the bare
//! 12 hex digits), so the core's overlay reader treats it like any other
//! boosts file. The last row for a (term, id) wins; hand-written overlays in
//! the config dir win over this file.

use crate::error::Result;
use crate::fsutil::{append_line, json_line, lock, read_optional};
use emoticond::{FaceId, Warning};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The boost a "doesn't fit" report writes (the scale is −3..=3).
pub const DEMOTE_BOOST: i8 = -3;

/// One `boosts.jsonl` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoostRow {
    /// The concept (`term_key`).
    pub term: String,
    /// Written as the bare 12 hex digits; `k`-prefixed ids are read too.
    #[serde(with = "hex_or_k")]
    pub id: FaceId,
    pub boost: i8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

mod hex_or_k {
    use emoticond::FaceId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &FaceId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&id.hex12())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<FaceId, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        FaceId::from_hex12(&s)
            .or_else(|| s.parse().ok())
            .ok_or_else(|| serde::de::Error::custom(format!("not a face id: {s:?}")))
    }
}

/// Read a boosts file: rows in file order, with bad lines as warnings.
/// A missing file is empty.
pub fn read_boosts(path: &Path) -> Result<(Vec<BoostRow>, Vec<Warning>)> {
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    let Some(b) = read_optional(path)? else { return Ok((rows, warnings)) };
    for (n, line) in String::from_utf8_lossy(&b).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<BoostRow>(line) {
            Ok(r) => rows.push(r),
            Err(e) => warnings.push(Warning::new(
                "overlay_bad_line",
                None,
                format!("{}:{}: {e}; line ignored", path.display(), n + 1),
            )),
        }
    }
    Ok((rows, warnings))
}

/// Demote `id` for `term` in the boosts overlay at `path`. Returns false if
/// the current row for that pair already says [`DEMOTE_BOOST`].
pub fn demote(path: &Path, term: &str, id: FaceId) -> Result<bool> {
    let _g = lock(path)?;
    let (rows, _) = read_boosts(path)?;
    if rows.iter().rev().find(|r| r.term == term && r.id == id).is_some_and(|r| r.boost == DEMOTE_BOOST) {
        return Ok(false);
    }
    let row = BoostRow { term: term.to_string(), id, boost: DEMOTE_BOOST, why: Some("doesn't fit (report)".into()) };
    append_line(path, &json_line(path, &row)?, true)?;
    Ok(true)
}

/// Undo [`demote`] (a cleared "doesn't fit" report): append a neutral row
/// (boost 0) for `(term, id)`, which wins as the last row. Returns false,
/// writing nothing, when the face is not demoted for that term.
pub fn undemote(path: &Path, term: &str, id: FaceId) -> Result<bool> {
    let _g = lock(path)?;
    let (rows, _) = read_boosts(path)?;
    if !rows.iter().rev().find(|r| r.term == term && r.id == id).is_some_and(|r| r.boost == DEMOTE_BOOST) {
        return Ok(false);
    }
    let row = BoostRow { term: term.to_string(), id, boost: 0, why: Some("report cleared".into()) };
    append_line(path, &json_line(path, &row)?, true)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn undemote_appends_a_neutral_row_once() {
        let d = TempDir::new();
        let p = d.path().join("overlays/boosts.jsonl");
        let id = FaceId::of_text("x");
        assert!(!undemote(&p, "shrug", id).unwrap());
        assert!(demote(&p, "shrug", id).unwrap());
        assert!(undemote(&p, "shrug", id).unwrap());
        assert!(!undemote(&p, "shrug", id).unwrap());
        let (rows, _) = read_boosts(&p).unwrap();
        assert_eq!(rows.last().unwrap().boost, 0);
        assert!(demote(&p, "shrug", id).unwrap());
    }

    #[test]
    fn demote_writes_shipped_format_once() {
        let d = TempDir::new();
        let p = d.path().join("overlays/boosts.jsonl");
        let id = FaceId::of_text("¯\\_(ツ)_/¯");
        assert!(demote(&p, "shrug", id).unwrap());
        assert!(!demote(&p, "shrug", id).unwrap());
        assert!(demote(&p, "idk", id).unwrap());
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            s.lines().next().unwrap(),
            r#"{"term":"shrug","id":"2026da3e4989","boost":-3,"why":"doesn't fit (report)"}"#
        );
        assert_eq!(s.lines().count(), 2);
    }

    #[test]
    fn reads_both_id_forms_and_warns_on_bad_lines() {
        let d = TempDir::new();
        let p = d.path().join("boosts.jsonl");
        std::fs::write(
            &p,
            "{\"term\":\"a\",\"id\":\"2026da3e4989\",\"boost\":2}\n{\"term\":\"a\",\"id\":\"k2026da3e4989\",\"boost\":-1}\nnope\n",
        )
        .unwrap();
        let (rows, w) = read_boosts(&p).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, rows[1].id);
        assert_eq!(w.len(), 1);
    }
}
