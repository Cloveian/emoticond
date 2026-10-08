//! The manifest (`MANI`): build-time facts about a data set, as UTF-8
//! `key=value` lines (docs/format.md §4).
//!
//! The engine reads its data-dependent constants from here (emotion names,
//! safety and crude thresholds, `dense_k`), so they ship with the data they
//! were calibrated on.

use serde::{Deserialize, Serialize};

/// A data set's manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Manifest {
    /// Every `key=value` line, in file order (the typed fields below are
    /// parsed from these).
    pub entries: Vec<(String, String)>,
    /// `format`: the format version the file was written as, `"3.0"`.
    pub format: String,
    /// `data_version`: the data release, `X.Y` (`1.0`), or `dev`.
    pub data_version: String,
    /// `set`: `full`, `core` or `lite`.
    pub set: String,
    /// `build_date`, `source_rev`, `policy`: provenance (may be empty).
    pub build_date: String,
    pub source_rev: String,
    pub policy: String,
    /// `licence`: SPDX id of the data licence (`CC-BY-4.0`); the text is in `LICN`.
    pub licence: String,
    pub n_faces: usize,
    pub n_terms: usize,
    pub n_phrases: usize,
    pub n_situations: usize,
    pub n_canonical: usize,
    /// Terms with curated boosts.
    pub n_boost_terms: usize,
    /// Neighbour-list length per vocab term (`DNSE`/`ENGN`).
    pub dense_k: usize,
    /// The emotion names, in the order of every profile.
    pub emotions: Vec<String>,
    /// The attribute names (`multi,cute,intensity,suggestive,lenny,face`).
    pub extras: Vec<String>,
    /// `quality_cut`: faces predicted below this were left out (none if absent).
    pub quality_cut: Option<f32>,
    /// `safety.innuendo_at`: suggestive at or above this is innuendo (penalised, flagged).
    pub innuendo_at: f32,
    /// `safety.sexual_at`: suggestive at or above this is hidden under `strict`.
    pub sexual_at: f32,
    /// `crude.at`: crude at or above this counts as crude.
    pub crude_at: f32,
    /// `long_at_default`: the default `long_at`.
    pub long_at_default: u16,
    /// `languages`: one `GRAM` section each.
    pub languages: Vec<String>,
    /// `phrase_max`: the longest phrase spelling, in tokens (at most 8).
    pub phrase_max: usize,
    /// `min_reader`: the lowest library version that reads every required section.
    pub min_reader: String,
}

fn list(v: &str) -> Vec<String> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

impl Manifest {
    /// Parse `MANI` bytes. `None` if they are not UTF-8 or a required key
    /// (`format`, `emotions`, `n_faces`, `n_terms`, `dense_k`) is missing or
    /// malformed. Unknown keys are kept in `entries` and otherwise ignored.
    pub fn parse(bytes: &[u8]) -> Option<Manifest> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=')?;
            entries.push((k.trim().to_string(), v.trim().to_string()));
        }
        let get = |k: &str| entries.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        let num = |k: &str| -> Option<usize> { get(k)?.parse().ok() };
        let num0 = |k: &str| num(k).unwrap_or(0);
        let float = |k: &str, d: f32| -> Option<f32> {
            match get(k) {
                None => Some(d),
                Some(v) => v.parse::<f32>().ok().filter(|x| x.is_finite()),
            }
        };
        let s = |k: &str| get(k).unwrap_or("").to_string();
        let m = Manifest {
            format: get("format")?.to_string(),
            data_version: s("data_version"),
            set: s("set"),
            build_date: s("build_date"),
            source_rev: s("source_rev"),
            policy: s("policy"),
            licence: s("licence"),
            n_faces: num("n_faces")?,
            n_terms: num("n_terms")?,
            n_phrases: num0("n_phrases"),
            n_situations: num0("n_situations"),
            n_canonical: num0("n_canonical"),
            n_boost_terms: num0("n_boost_terms"),
            dense_k: num("dense_k")?,
            emotions: list(get("emotions")?),
            extras: list(get("extras").unwrap_or("")),
            quality_cut: match get("quality_cut") {
                None | Some("") | Some("none") => None,
                Some(v) => Some(v.parse::<f32>().ok().filter(|x| x.is_finite())?),
            },
            innuendo_at: float("safety.innuendo_at", 0.5)?,
            sexual_at: float("safety.sexual_at", 1.5)?,
            crude_at: float("crude.at", 0.5)?,
            long_at_default: match get("long_at_default") {
                None => 14,
                Some(v) => v.parse().ok()?,
            },
            languages: list(get("languages").unwrap_or("")),
            phrase_max: num("phrase_max").unwrap_or(1).clamp(1, 8),
            min_reader: s("min_reader"),
            entries,
        };
        Some(m)
    }

    /// The raw value of a key (the last, if repeated).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_defaults() {
        let m = Manifest::parse(b"format=3.0\nemotions=happy, sad\nn_faces=2\nn_terms=1\ndense_k=10\n# c\nx.y=z\n").unwrap();
        assert_eq!(m.emotions, ["happy", "sad"]);
        assert_eq!((m.n_faces, m.n_terms, m.dense_k), (2, 1, 10));
        assert_eq!((m.innuendo_at, m.sexual_at, m.crude_at, m.long_at_default), (0.5, 1.5, 0.5, 14));
        assert_eq!(m.get("x.y"), Some("z"));
        assert_eq!(m.quality_cut, None);
    }

    #[test]
    fn refuses_bad_manifests() {
        assert!(Manifest::parse(b"format=3.0\n").is_none(), "required keys");
        assert!(Manifest::parse(b"format=3.0\nemotions=a\nn_faces=x\nn_terms=1\ndense_k=1").is_none());
        assert!(Manifest::parse(b"no equals sign").is_none());
        assert!(Manifest::parse(&[0xff, 0xfe]).is_none());
        assert!(Manifest::parse(b"format=3\nemotions=a\nn_faces=1\nn_terms=1\ndense_k=1\nsafety.sexual_at=NaN").is_none());
    }
}
