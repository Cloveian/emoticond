#![allow(dead_code)]

use emoticond::{FaceId, Reason, Report, ReportBuilder, Target};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A temp dir removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "emoticond-state-it-{}-{}-{}",
            std::process::id(),
            emoticond_state::now_ms(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn face(text: &str, rank: u32) -> Target {
    Target::Face { id: FaceId::of_text(text), text: text.to_string(), rank }
}

pub fn builder(query: &str, target: Target, reason: Reason) -> ReportBuilder {
    ReportBuilder::new(query, target, reason)
        .term_key(Some(query.trim().to_lowercase()))
        .reading(Some(format!("{query} · test reading")))
        .shown((0..25).map(|i| FaceId::of_text(&format!("shown{i}"))))
}

pub fn report(id: &str, ts: u64, query: &str, target: Target, reason: Reason) -> Report {
    builder(query, target, reason).stamp(id, ts, emoticond_state::DISCLAIMER_VERSION).build().unwrap()
}

/// A `Clear` withdrawing the choice `clears` (its slot) for query + target.
pub fn clear(id: &str, ts: u64, query: &str, target: Target, clears: Reason) -> Report {
    builder(query, target, Reason::Clear).clears(clears).stamp(id, ts, emoticond_state::DISCLAIMER_VERSION).build().unwrap()
}
