//! Blocklist files (docs/options.md §6.1).
//!
//! One face per line, either its id (`k2026da3e4989`, or the bare 12 hex
//! digits the shipped data files use) or its exact text. Leading and
//! trailing whitespace is trimmed, so a face whose text starts or ends with
//! a space must be listed by id. A line that is `#` alone or starts with `#`
//! and a space or tab is a comment (faces such as `#_#` are not). Empty
//! lines are ignored.
//!
//! The files merged for `OpenOptions::blocklist`: the machine-written
//! `$XDG_STATE_HOME/emoticond/blocklist.txt` (offensive reports), the user's
//! hand-written one in the config dir, and the packager's
//! `/etc/emoticond/blocklist.txt`. A missing file is not an error; a bad line
//! is a warning, never a failure.

use crate::error::Result;
use crate::fsutil::{append_line, lock, read_optional, write_atomic};
use emoticond::{FaceId, Warning};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The packager's / admin's blocklist.
pub const SYSTEM_BLOCKLIST: &str = "/etc/emoticond/blocklist.txt";

/// Longest line read as face text, in chars; longer lines are skipped.
const MAX_TEXT_CHARS: usize = 300;

const HEADER: &str = "# Faces hidden on this computer (kaomoji). One face id or exact face text per line.\n\
# Offensive reports add ids here. Delete a line to show that face again.";

/// The merged blocklists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Blocklist {
    pub ids: BTreeSet<FaceId>,
    pub warnings: Vec<Warning>,
}

enum Line {
    Skip,
    Face(FaceId),
    Bad(&'static str, String),
}

fn is_comment(l: &str) -> bool {
    l == "#" || l.starts_with("# ") || l.starts_with("#\t")
}

/// Looks like an attempt at an id (k + letters/digits only) but is not one.
fn looks_like_id(l: &str) -> bool {
    l.len() > 1 && l.starts_with('k') && l.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn parse_line(raw: &[u8]) -> Line {
    let Ok(s) = std::str::from_utf8(raw) else { return Line::Bad("blocklist_bad_utf8", "not UTF-8".into()) };
    let l = s.trim();
    if l.is_empty() || is_comment(l) {
        return Line::Skip;
    }
    if let Ok(id) = l.parse::<FaceId>() {
        return Line::Face(id);
    }
    if let Some(id) = FaceId::from_hex12(l) {
        return Line::Face(id);
    }
    if looks_like_id(l) {
        return Line::Bad("blocklist_bad_id", format!("not a face id: {l:?}"));
    }
    if l.chars().count() > MAX_TEXT_CHARS {
        return Line::Bad("blocklist_too_long", format!("line longer than {MAX_TEXT_CHARS} characters"));
    }
    Line::Face(FaceId::of_text(l))
}

/// Parse one blocklist's bytes. `source` names it in warnings.
pub fn parse_blocklist(bytes: &[u8], source: &Path) -> (Vec<FaceId>, Vec<Warning>) {
    let mut ids = Vec::new();
    let mut warnings = Vec::new();
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(bytes);
    for (n, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        match parse_line(raw) {
            Line::Skip => {}
            Line::Face(id) => ids.push(id),
            Line::Bad(code, msg) => {
                warnings.push(Warning::new(code, None, format!("{}:{}: {msg}; line ignored", source.display(), n + 1)))
            }
        }
    }
    (ids, warnings)
}

/// Load and merge blocklist files. Missing files are skipped silently;
/// unreadable files and bad lines become warnings.
pub fn load_blocklists<P: AsRef<Path>>(paths: &[P]) -> Blocklist {
    let mut out = Blocklist::default();
    for p in paths {
        let p = p.as_ref();
        match read_optional(p) {
            Ok(None) => {}
            Ok(Some(b)) => {
                let (ids, w) = parse_blocklist(&b, p);
                out.ids.extend(ids);
                out.warnings.extend(w);
            }
            Err(e) => out.warnings.push(Warning::new("blocklist_unreadable", None, e.to_string())),
        }
    }
    out
}

/// Add `id` to the blocklist at `path` (created with a header comment if
/// missing). Returns false if it was already listed (by id or text).
pub fn add_to_blocklist(path: &Path, id: FaceId) -> Result<bool> {
    add_to_blocklist_noted(path, id, None)
}

/// As [`add_to_blocklist`], with the face's text written as a `# ` comment
/// line above the id so the file stays readable by hand.
pub fn add_to_blocklist_noted(path: &Path, id: FaceId, text: Option<&str>) -> Result<bool> {
    let _g = lock(path)?;
    let existing = read_optional(path)?;
    if let Some(b) = &existing {
        if parse_blocklist(b, path).0.contains(&id) {
            return Ok(false);
        }
    }
    let mut line = String::new();
    match &existing {
        None => {
            line.push_str(HEADER);
            line.push('\n');
        }
        Some(b) if !b.is_empty() && !b.ends_with(b"\n") => line.push('\n'),
        _ => {}
    }
    if let Some(t) = text.map(|t| t.replace(['\n', '\r'], " ")).filter(|t| !t.trim().is_empty()) {
        line.push_str("# ");
        line.push_str(&t);
        line.push('\n');
    }
    line.push_str(&id.to_string());
    append_line(path, &line, true)?;
    Ok(true)
}

/// Remove every line naming `id` (by id or text) from the blocklist at
/// `path`, keeping comments and other lines. Returns whether any was
/// removed. ("Show this face again".)
pub fn remove_from_blocklist(path: &Path, id: FaceId) -> Result<bool> {
    let _g = lock(path)?;
    let Some(b) = read_optional(path)? else { return Ok(false) };
    let mut kept: Vec<&[u8]> = Vec::new();
    let mut removed = false;
    for raw in b.split(|c| *c == b'\n') {
        if matches!(parse_line(raw), Line::Face(x) if x == id) {
            removed = true;
        } else {
            kept.push(raw);
        }
    }
    if removed {
        write_atomic(path, &kept.join(&b'\n'))?;
    }
    Ok(removed)
}

/// The paths as owned `PathBuf`s (a convenience for `Watched`).
pub(crate) fn owned(paths: &[impl AsRef<Path>]) -> Vec<PathBuf> {
    paths.iter().map(|p| p.as_ref().to_path_buf()).collect()
}

impl Blocklist {
    /// Load from `paths` (see [`load_blocklists`]).
    pub fn load<P: AsRef<Path>>(paths: &[P]) -> Blocklist {
        load_blocklists(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn parses_ids_text_comments_and_bad_lines() {
        let shrug = FaceId::of_text("¯\\_(ツ)_/¯");
        let text = "# comment\n\n#\t also a comment\n\
            k2026da3e4989\n\
            2026da3e4989\n\
            (╯°□°)╯︵ ┻━┻  \r\n\
            #_#\n\
            k12345\n\
            kXYZ\n";
        let (ids, w) = parse_blocklist(text.as_bytes(), Path::new("b.txt"));
        assert_eq!(ids, [shrug, shrug, FaceId::of_text("(╯°□°)╯︵ ┻━┻"), FaceId::of_text("#_#")]);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|w| w.code == "blocklist_bad_id"));
        assert!(w[0].message.contains("b.txt:8"));
    }

    #[test]
    fn tolerates_bad_utf8_and_bom() {
        let mut b = "\u{feff}k2026da3e4989\n".as_bytes().to_vec();
        b.extend_from_slice(&[0xff, 0xfe, b'\n']);
        let (ids, w) = parse_blocklist(&b, Path::new("x"));
        assert_eq!(ids.len(), 1);
        assert_eq!(w[0].code, "blocklist_bad_utf8");
    }

    #[test]
    fn merges_files_and_skips_missing() {
        let d = TempDir::new();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        std::fs::write(&a, "(^‿^)\n").unwrap();
        std::fs::write(&b, "(^‿^)\nಠ_ಠ\n").unwrap();
        let bl = load_blocklists(&[a, b, d.path().join("missing.txt")]);
        assert_eq!(bl.ids.len(), 2);
        assert!(bl.warnings.is_empty());
    }

    #[test]
    fn add_and_remove() {
        let d = TempDir::new();
        let p = d.path().join("state/blocklist.txt");
        let id = FaceId::of_text("ಠ_ಠ");
        assert!(add_to_blocklist(&p, id).unwrap());
        assert!(!add_to_blocklist(&p, id).unwrap());
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("# "));
        assert_eq!(s.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>(), [id.to_string()]);
        // listed by text counts as listed
        std::fs::write(&p, "# mine\nಠ_ಠ").unwrap();
        assert!(!add_to_blocklist(&p, id).unwrap());
        let other = FaceId::of_text("(^‿^)");
        assert!(add_to_blocklist(&p, other).unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("# mine\nಠ_ಠ\n{other}\n"));
        assert!(remove_from_blocklist(&p, id).unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("# mine\n{other}\n"));
        assert!(!remove_from_blocklist(&p, id).unwrap());
    }
}
