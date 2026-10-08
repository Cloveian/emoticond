//! Stable face ids.
//!
//! A [`FaceId`] is the first 48 bits (12 hex digits) of the SHA-1 of the
//! face's exact UTF-8 text. It depends on nothing but the text, so the same
//! face has the same id in every build, data set and release. The string form
//! is `"k"` followed by 12 lowercase hex digits (`k2026da3e4989` is
//! `¯\_(ツ)_/¯`); [`Display`](std::fmt::Display) and [`FromStr`] round-trip.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// The id of one kaomoji: 48 bits of SHA-1 over its exact text.
///
/// `Ord` is numeric, which is also the order of the string forms. Ranking
/// breaks score ties by ascending `FaceId`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FaceId(u64);

/// Mask for the 48 bits an id uses.
const MASK: u64 = (1 << 48) - 1;

impl FaceId {
    /// The id of a face text. The text is hashed exactly as given: no
    /// trimming or normalisation.
    pub fn of_text(text: &str) -> FaceId {
        let digest = sha1_smol::Sha1::from(text.as_bytes()).digest().bytes();
        let mut v = 0u64;
        for b in &digest[..6] {
            v = (v << 8) | u64::from(*b);
        }
        FaceId(v)
    }

    /// From the raw 48-bit value; `None` if any of the top 16 bits is set.
    pub fn from_u64(v: u64) -> Option<FaceId> {
        (v & !MASK == 0).then_some(FaceId(v))
    }

    /// The raw 48-bit value.
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// From the 12 hex digits alone, without the `k` prefix: the form
    /// `data/boosts.jsonl` and `data/emoticons.jsonl` use. Lowercase only.
    pub fn from_hex12(s: &str) -> Option<FaceId> {
        if s.len() != 12 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }
        u64::from_str_radix(s, 16).ok().map(FaceId)
    }

    /// Either form: `"k"` + 12 hex (the string form) or the bare 12 hex
    /// digits (`data/boosts.jsonl`, overlay rows). Lowercase only.
    pub fn parse_any(s: &str) -> Option<FaceId> {
        FaceId::from_hex12(s.strip_prefix('k').unwrap_or(s))
    }

    /// The 12 hex digits without the `k` prefix.
    pub fn hex12(self) -> String {
        format!("{:012x}", self.0)
    }
}

impl fmt::Display for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "k{:012x}", self.0)
    }
}

impl fmt::Debug for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FaceId({self})")
    }
}

/// Why a string is not a face id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a face id (expected \"k\" + 12 lowercase hex digits): {0:?}")]
pub struct ParseFaceIdError(pub String);

impl FromStr for FaceId {
    type Err = ParseFaceIdError;

    /// Parses `"k"` + 12 lowercase hex digits.
    fn from_str(s: &str) -> Result<FaceId, ParseFaceIdError> {
        s.strip_prefix('k').and_then(FaceId::from_hex12).ok_or_else(|| ParseFaceIdError(s.to_string()))
    }
}

impl Serialize for FaceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FaceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<FaceId, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrug_matches_the_pipeline_hash() {
        // data/boosts.jsonl: {"term":"shrug","id":"2026da3e4989",...}
        let id = FaceId::of_text("¯\\_(ツ)_/¯");
        assert_eq!(id.to_string(), "k2026da3e4989");
        assert_eq!(id.hex12(), "2026da3e4989");
        assert_eq!(FaceId::from_hex12("2026da3e4989"), Some(id));
    }

    #[test]
    fn display_and_parse_round_trip() {
        for t in ["(^‿^)", "-⩊-", "", " ", "ಠ_ಠ"] {
            let id = FaceId::of_text(t);
            let s = id.to_string();
            assert_eq!(s.len(), 13);
            assert_eq!(s.parse::<FaceId>().unwrap(), id);
            assert!(id.as_u64() <= MASK);
            assert_eq!(FaceId::from_u64(id.as_u64()), Some(id));
        }
    }

    #[test]
    fn parse_any_takes_both_forms() {
        let id = FaceId::of_text("¯\\_(ツ)_/¯");
        assert_eq!(FaceId::parse_any("k2026da3e4989"), Some(id));
        assert_eq!(FaceId::parse_any("2026da3e4989"), Some(id));
        assert_eq!(FaceId::parse_any("kk2026da3e4989"), None);
        assert_eq!(FaceId::parse_any("K2026da3e4989"), None);
    }

    #[test]
    fn rejects_bad_strings() {
        for s in ["", "k", "2026da3e4989", "k2026da3e498", "k2026da3e49890", "K2026da3e4989", "k2026DA3E4989", "kxxxxxxxxxxxx", "e1f600"] {
            assert!(s.parse::<FaceId>().is_err(), "{s}");
        }
        assert_eq!(FaceId::from_u64(1 << 48), None);
    }

    #[test]
    fn order_is_numeric_and_matches_strings() {
        let mut ids: Vec<FaceId> = ["a", "b", "c", "d", "e", "f"].iter().map(|t| FaceId::of_text(t)).collect();
        ids.sort();
        let strs: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let mut sorted = strs.clone();
        sorted.sort();
        assert_eq!(strs, sorted);
    }

    #[test]
    fn serde_uses_the_string_form() {
        let id = FaceId::of_text("(^‿^)");
        let j = serde_json::to_string(&id).unwrap();
        assert_eq!(j, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<FaceId>(&j).unwrap(), id);
        assert!(serde_json::from_str::<FaceId>("\"nope\"").is_err());
    }
}
