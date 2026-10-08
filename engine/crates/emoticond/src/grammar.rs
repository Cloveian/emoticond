//! The query parser's word lists (docs/options.md §2.1), one set per
//! language, shipped in the data file's `GRAM` sections
//! (docs/format.md).
//!
//! A `GRAM` section is UTF-8 text: a `lang=<code>` line, then one line per
//! list, `<name>=<word>\t<word>...`. The names are those of the source files
//! (`data/grammar/<lang>.json`): `intensifiers`, `diminishers`,
//! `diminisher_phrases`, `negators`, `stopwords`, `lewd`, `single`, `pair`.
//! The parser consults the union of every language's lists.

/// Word lists the parser uses. Words are lowercase single tokens except
/// `two_word_diminishers` ("kind of").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Grammar {
    /// Language code (`en`, `ja`); empty for a union of several.
    pub lang: String,
    /// `very`, `so`, ... : the next term is read as intense.
    pub intensifiers: Vec<String>,
    /// `kinda`, `slightly`, ... : the next term is read as mild.
    pub diminishers: Vec<String>,
    pub two_word_diminishers: Vec<String>,
    /// `not`, `never`, ... : the next feeling is negated.
    pub negators: Vec<String>,
    /// Filler words that carry no meaning on their own.
    pub stopwords: Vec<String>,
    /// Words that ask for suggestive faces (`lewd`, `nsfw`).
    pub lewd: Vec<String>,
    /// Words asking for a single figure (`solo`).
    pub single_words: Vec<String>,
    /// Words asking for two figures (`together`).
    pub pair_words: Vec<String>,
    /// Typed emoticons and the concept each stands for, `"<glyphs> <concept>"`
    /// (`":/ unsure"`, `"T_T crying"`): read before glyph search.
    pub emoticons: Vec<String>,
}

/// The list names, as in `data/grammar/<lang>.json` and `GRAM` sections.
pub const LISTS: [&str; 9] =
    ["intensifiers", "diminishers", "diminisher_phrases", "negators", "stopwords", "lewd", "single", "pair", "emoticons"];

impl Grammar {
    /// An empty set of lists for a language.
    pub fn new(lang: &str) -> Grammar {
        Grammar { lang: lang.to_string(), ..Grammar::default() }
    }

    /// The list with a [`LISTS`] name.
    pub fn list_mut(&mut self, name: &str) -> Option<&mut Vec<String>> {
        Some(match name {
            "intensifiers" => &mut self.intensifiers,
            "diminishers" => &mut self.diminishers,
            "diminisher_phrases" => &mut self.two_word_diminishers,
            "negators" => &mut self.negators,
            "stopwords" => &mut self.stopwords,
            "lewd" => &mut self.lewd,
            "single" => &mut self.single_words,
            "pair" => &mut self.pair_words,
            "emoticons" => &mut self.emoticons,
            _ => return None,
        })
    }

    /// The concept a typed emoticon stands for (`:/` -> `unsure`): an exact
    /// match first, then one ignoring ASCII case (`xd`, `XD`).
    pub fn emoticon(&self, s: &str) -> Option<&str> {
        let find = |exact: bool| {
            self.emoticons.iter().find_map(|e| {
                let (g, c) = e.split_once(' ')?;
                let hit = if exact { g == s } else { g.eq_ignore_ascii_case(s) };
                (hit && !c.trim().is_empty()).then(|| c.trim())
            })
        };
        find(true).or_else(|| find(false))
    }

    /// The list with a [`LISTS`] name (empty for an unknown name).
    pub fn list(&self, name: &str) -> &[String] {
        match name {
            "intensifiers" => &self.intensifiers,
            "diminishers" => &self.diminishers,
            "diminisher_phrases" => &self.two_word_diminishers,
            "negators" => &self.negators,
            "stopwords" => &self.stopwords,
            "lewd" => &self.lewd,
            "single" => &self.single_words,
            "pair" => &self.pair_words,
            "emoticons" => &self.emoticons,
            _ => &[],
        }
    }

    /// Parse a `GRAM` section. `None` if it is not UTF-8, has no `lang`, or
    /// has a line that is not `key=value`. Unknown lists are ignored (a
    /// newer minor may add some).
    pub fn parse(bytes: &[u8]) -> Option<Grammar> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut g = Grammar::default();
        let mut lang = None;
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once('=')?;
            if k == "lang" {
                lang = Some(v.to_string());
                continue;
            }
            if let Some(list) = g.list_mut(k) {
                list.extend(v.split('\t').filter(|w| !w.is_empty()).map(String::from));
            }
        }
        g.lang = lang.filter(|l| !l.is_empty())?;
        Some(g)
    }

    /// The `GRAM` section text (the inverse of [`parse`](Self::parse)).
    /// Words must not contain tabs or line breaks.
    pub fn to_section(&self) -> String {
        let mut s = format!("lang={}\n", self.lang);
        for name in LISTS {
            s.push_str(name);
            s.push('=');
            s.push_str(&self.list(name).join("\t"));
            s.push('\n');
        }
        s
    }

    /// Add another language's words (the parser reads the union).
    pub fn merge(&mut self, other: &Grammar) {
        for name in LISTS {
            if let Some(l) = self.list_mut(name) {
                for w in other.list(name) {
                    if !l.contains(w) {
                        l.push(w.clone());
                    }
                }
            }
        }
        if self.lang != other.lang {
            self.lang.clear();
        }
    }

    pub(crate) fn has(list: &[String], w: &str) -> bool {
        list.iter().any(|x| x == w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_round_trips() {
        let mut g = Grammar::new("en");
        g.intensifiers = vec!["very".into(), "so".into()];
        g.two_word_diminishers = vec!["kind of".into()];
        let back = Grammar::parse(g.to_section().as_bytes()).unwrap();
        assert_eq!(back, g);
        assert!(Grammar::parse(b"intensifiers=very\n").is_none(), "no lang");
        assert!(Grammar::parse(b"lang=en\nnonsense\n").is_none());
    }

    #[test]
    fn emoticons_map_to_concepts() {
        let mut g = Grammar::new("en");
        g.emoticons = vec![":/ unsure".into(), "xD laughing".into(), "T_T crying".into()];
        assert_eq!(g.emoticon(":/"), Some("unsure"));
        assert_eq!(g.emoticon("XD"), Some("laughing"), "case-insensitive fallback");
        assert_eq!(g.emoticon("t_t"), Some("crying"));
        assert_eq!(g.emoticon(":)"), None);
        let back = Grammar::parse(g.to_section().as_bytes()).unwrap();
        assert_eq!(back.emoticons, g.emoticons, "spaces inside an entry survive the section");
    }

    #[test]
    fn merge_is_a_union() {
        let mut a = Grammar::new("en");
        a.negators = vec!["not".into()];
        let mut b = Grammar::new("ja");
        b.negators = vec!["ない".into(), "not".into()];
        a.merge(&b);
        assert_eq!(a.negators, ["not", "ない"]);
        assert_eq!(a.lang, "");
    }
}
