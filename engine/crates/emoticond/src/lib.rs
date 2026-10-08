//! emoticond: a (legitimately) clever kaomoji search engine.
//!
//! The emotion engine (`emo`) over a data file (`data`: format 3, `.kmj`,
//! docs/format.md), and the public types of the library API (docs/api-frontends.md §1).
//!
//! The library is pure: it reads no env vars or config, has no clock or RNG,
//! and prints nothing. Everything that changes results arrives as
//! [`OpenOptions`] (once) or [`SearchOptions`] (per query).

/// This library's version (the `engine` in report and batch stamps):
/// `X.Y.Z`, where X is the data compatibility number (it opens data `X.*`),
/// Y the data release it goes with (data `X.Y`), and Z changes that leave
/// the data alone.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The data compatibility number: this library opens data versions
/// `DATA_COMPAT.*` (and unversioned development data).
pub const DATA_COMPAT: &str = env!("CARGO_PKG_VERSION_MAJOR");

/// Whether this library opens data with version `v` (`X.Y`): its X is
/// [`DATA_COMPAT`]. Development data (`dev`, or anything not `X.Y`) opens too.
pub fn data_compatible(v: &str) -> bool {
    match v.split_once('.') {
        Some((x, y)) if !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()) && !y.is_empty() && y.bytes().all(|b| b.is_ascii_digit()) => {
            x.trim_start_matches('0') == DATA_COMPAT.trim_start_matches('0')
        }
        _ => true,
    }
}

pub mod data;
pub mod db;
pub mod emo;
pub mod error;
pub mod id;
pub mod options;
pub mod overlay;
pub mod policy;
pub mod report;
pub mod result;
pub mod grammar;
pub mod text;
pub mod usage;

pub use error::{Level, OpenError, ParseOptionError, ReportError, Warning};
pub use id::{FaceId, ParseFaceIdError};
#[cfg(feature = "unstable-tuning")]
pub use options::Tuning;
pub use options::{
    Dataset, Dedupe, EmotionFilter, Explain, Figures, GlyphSearch, Intensity, Lang, OpenOptions,
    OverlaySource, Safety, SearchOptions, StyleMode, Styles, UsageWeight,
};
pub use overlay::{CanonPin, Overlay, OverlayBoost, OverlayKind, OverlayPhrase, OVERLAY_FILES};
pub use policy::{Policy, PopularityMode, StyleLocks};
pub use report::{report_key, EngineStamp, LocalEffect, Reason, Report, ReportBuilder, Target};
pub use result::{
    Attrs, Completion, CompletionSource, DbInfo, Entry, Flags, Hit, HitWhy, Modifier, ReadMode, ReadTerm, Reading,
    SearchResult, TermSource,
};
pub use usage::{Pick, UsageMap, UsageState};
pub use db::Database;
pub use grammar::Grammar;
pub use data::{DataBytes, DataFile, IdStatus, Manifest};
pub use emo::tokens;

#[cfg(test)]
mod version_tests {
    #[test]
    fn data_compatibility_is_the_first_number() {
        let x = super::DATA_COMPAT;
        assert!(super::data_compatible(&format!("{x}.0")));
        assert!(super::data_compatible(&format!("{x}.7")));
        assert!(!super::data_compatible(&format!("{}.0", x.parse::<u32>().unwrap() + 1)));
        assert!(super::data_compatible("dev"));
        assert!(super::data_compatible("2026.10.1x"));
    }
}
