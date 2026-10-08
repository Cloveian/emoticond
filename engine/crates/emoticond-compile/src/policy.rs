//! The licence policy a public build is checked against (docs/public-data.md).
//!
//! The export labels every word on a face with its provenance classes (`wc`,
//! parallel to `w` in `faces.jsonl`) and every vocab key with its evidence
//! (`ev` in `vocab.jsonl`). A public build trusts those labels,
//! and refuses the export if any label is a class this policy does
//! not allow, if a label is missing, or if the export's `meta.json` does
//! not say `policy: public`. The data pipeline's audit re-derives the
//! labels from its raw inputs on its own; this is the compiler's half.
//!
//! The policy is fixed here, not read from the export, so a public build
//! cannot be loosened by its input. Its text is hashed into the manifest's
//! `policy=public@<digest>`.

use std::collections::BTreeMap;

/// The public policy, as text: what `public@<digest>` names.
pub const PUBLIC_POLICY: &str = "\
kaomoji public data policy 1
# classes a word on a face may carry (pipeline/public_policy.py face_class_ok)
face a m l o s:kmoji s:kaomojiru s:fontvibe s:emojicombos
# evidence a vocab key may carry (vocab_class_ok)
vocab own:cat own:lexicon own:agent own:model own:corpus own:phrase own:situation own:canonical own:gloss u:glyph ja:reduced s:kmoji s:kaomojiru s:fontvibe s:emojicombos
";

fn line(kind: &str) -> impl Iterator<Item = &'static str> {
    PUBLIC_POLICY
        .lines()
        .find(|l| l.split_whitespace().next() == Some(kind))
        .into_iter()
        .flat_map(|l| l.split_whitespace().skip(1))
}

/// Whether a face word's class may ship.
pub fn face_class_ok(class: &str) -> bool {
    line("face").any(|c| c == class)
}

/// Whether a vocab key's evidence class may ship.
pub fn vocab_class_ok(class: &str) -> bool {
    line("vocab").any(|c| c == class)
}

/// `public@<16 hex>`: the manifest's policy value for a public build.
pub fn public_policy_id() -> String {
    format!("public@{}", &crate::sha256_hex(&[PUBLIC_POLICY.as_bytes()])[..16])
}

/// What a public check found wrong, by class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Violations {
    /// face word classes not allowed, with counts
    pub face: BTreeMap<String, usize>,
    /// vocab evidence classes not allowed, with counts
    pub vocab: BTreeMap<String, usize>,
    /// faces whose words carry no (or too few) labels
    pub faces_unlabelled: usize,
    /// vocab keys with no evidence
    pub keys_unlabelled: usize,
    /// what the export's meta.json says, if not `public`
    pub meta_policy: Option<String>,
}

impl Violations {
    pub fn is_empty(&self) -> bool {
        *self == Violations::default()
    }

    /// A face's `wc` labels (one per word, comma-separated classes).
    pub fn check_face(&mut self, n_words: usize, wc: Option<&[String]>) {
        match wc {
            Some(l) if l.len() == n_words => {
                for c in l.iter().flat_map(|x| x.split(',')).map(str::trim) {
                    if !face_class_ok(c) {
                        *self.face.entry(c.to_string()).or_default() += 1;
                    }
                }
            }
            _ if n_words == 0 => {}
            _ => self.faces_unlabelled += 1,
        }
    }

    /// A vocab key's `ev` evidence.
    pub fn check_key(&mut self, ev: Option<&[String]>) {
        match ev {
            Some(l) if !l.is_empty() => {
                for c in l {
                    if !vocab_class_ok(c) {
                        *self.vocab.entry(c.clone()).or_default() += 1;
                    }
                }
            }
            _ => self.keys_unlabelled += 1,
        }
    }

    /// The refusal message: every class and count.
    pub fn report(&self) -> String {
        let mut m = vec!["the export breaks the public policy:".to_string()];
        if let Some(p) = &self.meta_policy {
            m.push(format!("  meta.json policy is {p:?}, not \"public\" (export with --policy public)"));
        }
        for (c, n) in &self.face {
            m.push(format!("  face words of class {c}: {n}"));
        }
        for (c, n) in &self.vocab {
            m.push(format!("  vocab evidence {c}: {n}"));
        }
        if self.faces_unlabelled > 0 {
            m.push(format!("  faces without provenance labels (wc): {}", self.faces_unlabelled));
        }
        if self.keys_unlabelled > 0 {
            m.push(format!("  vocab keys without evidence (ev): {}", self.keys_unlabelled));
        }
        m.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        for c in ["a", "m", "l", "o", "s:kmoji", "s:emojicombos", "s:fontvibe"] {
            assert!(face_class_ok(c), "{c}");
        }
        for c in ["s:ekohrt", "s:emojicombos/multi", "s:fontvibe/kaosute", "s:gsozai", "s:kaomojikuma", "", "own:cat"] {
            assert!(!face_class_ok(c), "{c}");
        }
        for c in ["own:cat", "own:gloss", "u:glyph", "ja:reduced", "s:kmoji"] {
            assert!(vocab_class_ok(c), "{c}");
        }
        for c in ["list:emojidb", "ja:lexicon", "s:ekohrt", "own:other", "face"] {
            assert!(!vocab_class_ok(c), "{c}");
        }
        assert!(public_policy_id().starts_with("public@") && public_policy_id().len() == 23);
    }

    #[test]
    fn violations_count_by_class() {
        let mut v = Violations::default();
        v.check_face(2, Some(&["a,m".into(), "s:ekohrt".into()]));
        v.check_face(2, Some(&["a".into()]));
        v.check_face(0, None);
        v.check_key(Some(&["own:cat".into(), "list:emojidb".into()]));
        v.check_key(Some(&[]));
        assert_eq!(v.face.get("s:ekohrt"), Some(&1));
        assert_eq!(v.vocab.get("list:emojidb"), Some(&1));
        assert_eq!((v.faces_unlabelled, v.keys_unlabelled), (1, 1));
        assert!(v.report().contains("s:ekohrt: 1"));
    }
}
