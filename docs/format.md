# The kaomoji data file (`.kmj`), format 3

Status: normative for formats 3.0 and 3.1. The reader is `emoticond::data`
(`engine/crates/emoticond/src/data/`), the writer is `emoticond-compile`; both
share the record definitions in `emoticond::data::format`, so this text and
that module must agree. The format is kaomoji only: there are no emoji or
symbol sections.

## 1. Overview

One file holds one data set (`full`, `core` or `lite`): every face, the
vocabulary, the precomputed neighbour lists, tag postings, the compiled
phrases, situations, canonical picks and boosts, the parser's grammar word
lists, the manifest, the data licence and attribution.

- **Little-endian** throughout. Readers on big-endian hosts refuse the file
  (`OpenError::BigEndian`).
- **Zero copy.** Every section starts at an 8-aligned offset and holds
  fixed-size records of 4- or 8-byte fields with no padding, so a reader
  with an 8-aligned buffer (a memory map, an aligned `Vec`,
  `include_data!`) reads sections in place. The compact encodings of 3.1
  (§5.3) are read in place too and decoded as they are used; the one
  exception is the coded face numbers, which the reader decodes once, on
  the first search (scoring reads every face's numbers on every query).
- **Nothing is parsed at open** except the manifest and grammar text (a
  few KB). No JSON is involved in reading.
- **Replace by rename.** Writers write a temp file beside the target and
  rename it over. A file that is mapped must never be rewritten in place
  (readers may crash with SIGBUS).
- **Deterministic.** The same sources compile to the same bytes. Nothing
  depends on time, hashing order or the machine; `build_date` is recorded
  only when the builder passes one.

## 2. Header

| Offset | Size | Field | |
|---|---|---|---|
| 0 | 8 | magic | `"KAOMOJI\x1a"` |
| 8 | 2 | format_major | `3`. A reader refuses any other major (`Incompatible`). |
| 10 | 2 | format_minor | `0` or `1` (§6). A reader accepts newer minors. |
| 12 | 4 | flags | bit 0: built under the public licence policy (`--policy public`); bit 1: has `ALIA`. Others 0. |
| 16 | 8 | header_len | `64 + 32 × n_sections`: the fixed header plus the directory |
| 24 | 4 | n_sections | at most 4096 |
| 28 | 4 | reserved | 0 |
| 32 | 32 | content_hash | SHA-256 of bytes `64..` (directory and sections). Identifies the data exactly (`DbInfo::content_hash`, reports' engine stamp). Not checked at open. |
| 64 | 32 × n | directory | one entry per section, in file order |

The data release (`data_version`) is in the manifest (§4), not the header:
a reader only needs the format version to decide whether it can read a file.

### 2.1 Directory entry (32 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | tag (four ASCII bytes) |
| 4 | 4 | flags: bit 0 = required |
| 8 | 8 | offset of the section from the start of the file; a multiple of 8, ≥ header_len |
| 16 | 8 | length in bytes; a multiple of elem_size |
| 24 | 4 | elem_size: the record size |
| 28 | 4 | crc32 of the section's bytes (IEEE, as zlib) |

Sections do not overlap and are laid out in directory order, each padded
with zero bytes to the next multiple of 8; the file ends padded to a
multiple of 8.

A reader:

- refuses a file whose directory has an entry with an unknown tag and the
  required flag (`Incompatible`); skips unknown optional sections;
- refuses (`Corrupt { section }`) a known section with the wrong
  `elem_size`, a misaligned offset, bounds outside the file, a length that
  is not a whole number of records, a repeat (every tag but `GRAM` occurs
  at most once), a required section missing (for a table with a 3.1
  alternative, §5.3: neither encoding present, or both), `FCHR` without
  `CHRS` or the reverse, or a compact section whose header is out of range
  (§5.3);
- checks the cross-section lengths of §5 and the manifest's counts;
- does **not** scan section contents. Readers bound-check every span,
  ordinal and range when they use it and read invalid UTF-8 as `""`, so a
  damaged section degrades results and never crashes. `Database::verify()`
  checks every crc32 on demand.

## 3. Common encodings

- **Span** `[u32; 2]`: a byte range `[a, b)` in `STRS`, the string heap
  (UTF-8, no terminators).
- **List range** `[u32; 2]`: a range `[a, b)` of entries in `SLST`, whose
  entries are spans. Used for word lists and tokenised spellings.
- **Ordinal** `u32`: a face's position in the face tables. Faces are stored
  sorted by `FaceId`, so ordinal order is id order. `0xFFFF_FFFF` (`NONE`)
  means no face. Ordinals are internal: they are never exposed or stored
  outside the file.
- **FaceId**: 48 bits of SHA-1 over the face's exact UTF-8 text, stored in
  a `u64`. Written `k` + 12 hex digits (`k2026da3e4989`) outside the file.
- **Profiles** are `[f32; 19]` in the manifest's `emotions` order, summing
  to 1 (or all zero). The compiler rejects NaN and infinities everywhere.
- **Tokens**: spellings of phrases and situations are tokenised with
  `emoticond::tokens` (lowercase; letters, digits, `'` and `$` kept, all else
  separates) and joined with single spaces.

## 4. Text sections

| Tag | Req | Content |
|---|---|---|
| `MANI` | yes | the manifest, below |
| `LICN` | yes | the data licence text (UTF-8) |
| `ATTR` | yes | attribution and notices for the sources (UTF-8) |
| `GRAM` | yes, ≥ 1 | one per language: the parser's word lists, below |

### 4.1 `MANI`

UTF-8 lines `key=value`; blank lines and lines starting with `#` are
ignored; values never contain line breaks; lists are comma-separated.
Unknown keys are kept (`Manifest::entries`) and otherwise ignored.

| Key | Req | Example | Meaning |
|---|---|---|---|
| `format` | yes | `3.0` | the format the file was written as |
| `data_version` | | `1.0`, `dev` | the data release, `X.Y`, or `dev` for a development build. A library `X.Y.Z` opens data `X.*` (and `dev`); other majors are refused (`Incompatible`) |
| `set` | | `full` | `full`, `core` or `lite` |
| `build_date` | | `2026-10-06` or empty | as given to the compiler |
| `source_rev` | | 16 hex | digest of the compiler's inputs |
| `policy` | | `private`, `public@<digest>` | licence policy the build passed (`<digest>`: the first 16 hex of SHA-256 over the compiler's policy text, `emoticond_compile::policy::PUBLIC_POLICY`) |
| `licence` | | `CC-BY-4.0` | SPDX id of the data licence (`LICN` has the text) |
| `n_faces`, `n_terms` | yes | | must equal the `FIDS` and `TERM` counts |
| `n_phrases`, `n_situations`, `n_canonical` | | | must equal the `PHRS`, `SITU` and `CANO` counts (0 if absent) |
| `n_boost_terms` | | | terms with boosts |
| `dense_k` | yes | `100` | neighbour-list length per term |
| `emotions` | yes | `happy,laughing,…` | exactly 19 names: the order of every profile |
| `extras` | | `multi,cute,intensity,suggestive,lenny,face` | attribute names |
| `quality_cut` | | `none`, `3` | faces below this predicted quality were left out by the export |
| `safety.innuendo_at` | | `0.5` | suggestive ≥ this: innuendo (flagged `SUGGESTIVE`, penalised unless asked) |
| `safety.sexual_at` | | `1.5` | suggestive ≥ this: hidden under `safety = strict` (flagged `EXPLICIT`) |
| `crude.at` | | `0.5` | crude ≥ this counts as crude |
| `long_at_default` | | `14` | the default for `long_at` |
| `languages` | | `en,ja` | the `GRAM` sections present |
| `phrase_max` | | `8` | the longest phrase spelling in tokens (1..8) |
| `min_reader` | | `0.1.0` | the lowest library version that reads every required section |
| `encoding` | | `nums=q16,lists=packed,postings=varint,chars=derived,strings=dedupe` | 3.1 only: how the tables are stored (§5.3); informational, readers go by the sections present |
| `n_aliases`, `n_retired` | | `0` | 3.1 only: `ALIA` and `RETD` counts |

Thresholds absent from the manifest default to the values in the example
column. They ship with the data because they are calibrated to its scales.

### 4.2 `GRAM`

UTF-8. A line `lang=<code>`, then one line per list,
`<name>=<word>\t<word>…` (words never contain tabs or line breaks). Lists:
`intensifiers`, `diminishers`, `diminisher_phrases` (two-word: `kind of`),
`negators`, `stopwords`, `lewd`, `single`, `pair`, `emoticons` (typed
emoticons and the concept each stands for, `<glyphs> <concept>`: `:/
unsure`, `T_T crying`; read before glyph search). Unknown lists are
ignored, and a missing list is empty (files before `emoticons` was added
still open). The parser uses the union of every language's lists. The
sources are `data/grammar/<lang>.json` with the same list names.

## 5. Table sections

`n` = number of faces, `t` = number of vocab terms, `k` = `dense_k`.

| Tag | Req | elem | Count | Content |
|---|---|---|---|---|
| `STRS` | yes | 1 | | the string heap |
| `FNUM` | yes¹ | 116 | n | `FaceNum` (below) |
| `FIDS` | yes | 8 | n | `u64` FaceId per face, **strictly ascending** |
| `FTXT` | yes | 8 | n | span of the face's text |
| `FCHR` | yes² | 8 | n | range `[a, b)` into `CHRS`: the face's distinct non-space chars |
| `CHRS` | yes² | 4 | | `u32` code points, sorted within each face (near-duplicate test) |
| `BTXT` | yes | 4 | n | ordinals sorted by text bytes (exact text lookup) |
| `TERM` | yes | 92 | t | `TermRec` per vocab line, in export order (a key may repeat) |
| `TKEY` | yes | 8 | t | span of each term's key |
| `TSRT` | yes | 4 | ≤ t | term indices sorted by key, one per distinct key (the last line with a key wins) |
| `DNSE` | yes¹ | 4 | t × k | per term, k ordinals: the e5 model's nearest faces, best first; `NONE` pads, and marks a neighbour that is not in this set (its slot keeps the ranks after it) |
| `ENGN` | no | 4 | t × k | the same from the affect engine (every set has it: `lite` without it fails its quality gate, docs/public-data.md) |
| `WKEY` | yes | 8 | w | spans of the tag words, sorted by bytes |
| `WPST` | yes | 8 | w | per word, a range `[a, b)` into `POST` |
| `POST` | yes¹ | 4 | | `ordinal << 2 \| tier` (tier 0 best .. 2), ascending ordinal within a word |
| `SLST` | yes | 8 | | spans: the strings that list ranges point at |
| `PHRS` | yes | 104 | | `PhraseRec`, in source order |
| `PIDX` | yes | 12 | | `[a, b, phrase]`: every spelling (tokens joined by spaces; the key is one) and its phrase, sorted by bytes; the first phrase with a spelling wins |
| `SITU` | yes | 104 | | `SituRec`, sorted by name |
| `SMAT` | yes | 8 | | list ranges: each situation spelling as tokens |
| `CANO` | yes | 24 | | `CanonRec`, sorted by term |
| `BOST` | no | 24 | | `BoostRec`, sorted by (term, id) |

| `ALIA` | no | 16 | | 3.1: `[old: u64, new: u64]` FaceIds, sorted by old (§5.4) |
| `RETD` | no | 16 | | 3.1: `[id: u64, reason: u64]`, sorted by id (§5.4) |

¹ In 3.1, `FNUM`, `DNSE`/`ENGN` and `POST` may instead be stored compact
(§5.3): exactly one of `FNUM`, `FNQ2`, `FNQ1`; one of `DNSE`, `DNSP` (and
at most one of `ENGN`, `ENGP`); one of `POST`, `PSTV`.
² In 3.1, `FCHR` and `CHRS` may both be left out; the reader then derives
a face's char set from its text (`emoticond::text::char_set`, the same
function the compiler uses), so results do not change.

Reserved for later minors (optional): `WCLS` (per-posting provenance).

### 5.1 Records

All fields 4 bytes, little-endian, in this order, no padding.

`FaceNum` (116 bytes):
`r: [f32; 19]` emotion intensities 0..1; `multi`, `cute`, `intensity`
(0..1); `suggestive` (0..3); `lenny`; `face` (0..1, how much it reads as a
face); `quality` (predicted 1..8); `crude: f32` (0..1); `len: u32` (chars);
`flags: u32` (reserved, 0).

`TermRec` (92 bytes):
`p: [f32; 19]`; `m: f32` expected share of multi-figure faces;
`n: u32` faces carrying the term; `tier: u32` (2 emotion word, 1
hand-written lexicon key, 0 tag phrase); `flags: u32` (bit 0: the profile
comes only from e5's neighbours, "weak"; bit 1: `m` is known).

`PhraseRec` (104 bytes):
`p: [f32; 19]`; `key: span`; `words: list range` (tag words);
`level: u32` (1 mild, 2 plain, 3 strong; `0xFFFF_FFFF` unset);
`pair: u32` (0 single, 1 pair, `0xFFFF_FFFF` unset);
`flags: u32` (bit 0 cute, bit 1 lenny, bit 2 suggestive, bit 3 `over`:
the phrase beats a vocab term of the same spelling however well the data
knows the word; otherwise a phrase only replaces a term the data knows
weakly: e5-only, a tag profile from under 100 faces, or a tag+engine
profile from under 50).

`SituRec` (104 bytes):
`p: [f32; 19]`; `name: span`; `matches: [u32; 2]` range into `SMAT`;
`words: list range`; `pair: u32` as above.

`CanonRec` (24 bytes):
`term: span` (lowercase); `faces: [u32; 3]` ordinals best first, `NONE`
after the last; `n: u32` (1..3). The compiler ranks hand picks before
agent picks, then by the source's rank, then by export order, drops
repeats and keeps three.

`BoostRec` (24 bytes):
`term: span`; `id: [u32; 2]` the FaceId's low and high 32 bits;
`boost: f32` (−3..3); `reserved: u32` (0). Boosts are keyed by id, so a
boost for a face missing from this set is harmless.

### 5.2 Cross-section invariants (checked at open)

`FIDS` = `FNUM` = `FTXT` = `FCHR` = `BTXT` = `n_faces` < 2³⁰;
`TERM` = `TKEY` = `n_terms`; `TSRT` ≤ `n_terms`; `DNSE` = t × k;
`ENGN` = t × k or absent; `WPST` = `WKEY`; `PHRS` = `n_phrases`;
`SITU` = `n_situations`; `CANO` = `n_canonical`; 19 emotion names.
In 3.1 the face count also holds for `FNQ2`/`FNQ1` (in place of `FNUM`)
and for `FCHR` when present; `DNSP`/`ENGP` must name `n_terms` terms.

### 5.3 Compact encodings (format 3.1)

Each replaces a 3.0 table with the same content in fewer bytes. All but
`FNQ1` are lossless: a file using them answers every query exactly as the
3.0 layout does (golden identical, docs/public-data.md). The compact
sections are flagged required, so a 3.0 reader refuses such a file as
`Incompatible` rather than misreading it.

| Tag | Req | elem | Replaces | Content |
|---|---|---|---|---|
| `FNQ2` | yes | 56 | `FNUM` | per face: 27 `u16` codes, then `len: u16` |
| `FNQ1` | yes | 30 | `FNUM` | per face: 27 `u8` codes, a zero byte, then `len: u16` |
| `FNCB` | yes | 4 | | the code tables of `FNQ2`/`FNQ1` |
| `DNSP` | yes | 4 | `DNSE` | packed neighbour lists |
| `ENGP` | yes | 4 | `ENGN` | packed affect-engine lists |
| `PSTV` | yes | 1 | `POST` | varint postings |

**Coded face numbers** (`FNQ2`, `FNQ1` with `FNCB`). The 27 coded fields
are, in order, `r[0..19]`, `multi`, `cute`, `intensity`, `suggestive`,
`lenny`, `face`, `quality`, `crude` (`FaceNum` without `len` and `flags`;
`len` is stored as a `u16`, saturating, and `flags` is 0). `FNCB` is `u32`
words: 28 table starts `s[0..=27]`, then f32 values; field `f`'s table is
values `s[f]..s[f+1]`, and a code `c` decodes to entry `c` of its field's
table (0.0 if out of range).

- `FNQ2` (`--nums q16`): each field's table holds every distinct value
  (by bit pattern) in ascending order, so decoding is exact, bit for bit.
  The compiler refuses a field with more than 65,536 distinct values. Real
  data has at most ~3,000 (three decimals).
- `FNQ1` (`--nums q8`, lossy): a field with at most 256 distinct values is
  stored exactly as above. Otherwise its range is cut into uniform bins
  (255, less two per threshold), and a bin's value is the mean of the
  values in it, weighted by faces. Bins never straddle a threshold the
  engine tests: `multi` 0.5, `suggestive` at `safety.innuendo_at` and
  `safety.sexual_at`, `lenny` 0.5, `face` 0.5, `crude` at `crude.at`. A
  value equal to a threshold gets a bin of its own. So every flag
  (`SUGGESTIVE`, `EXPLICIT`, `LENNY`, `CRUDE`, `MULTI`, `NOT_FACE`) and
  every hard filter on those fields is unchanged; only scores move, by less
  than one bin (a field's range / ~250).

**Packed neighbour lists** (`DNSP`, `ENGP`): `u32` words. `w[0]` = `bits`
(1..=32), `w[1]` = the number of terms (must equal `TERM`'s count), then
`t + 1` slot starts, then the slots, `bits` wide each, packed
little-endian from bit 0 of the word after the starts (slot `j` occupies
bits `j·bits .. (j+1)·bits` of that bit stream). Term `i`'s list is slots
`start[i] .. start[i+1]`, read in rank order; a reader caps it at
`dense_k`. All ones means `NONE`. `bits` is the bit length of the number
of faces, so every ordinal and the `NONE` code fit (17 bits for core's
81k faces, 18 for full). Trailing `NONE` slots are not stored; interior
ones are, so ranks are kept.

**Varint postings** (`PSTV`): per tag word, `WPST`'s range is a byte range
of `PSTV` holding, per posting in ascending ordinal order, the LEB128
varint of `(ordinal − previous ordinal) << 2 | tier` (the first gap is
from 0).

**Derived char sets.** Without `FCHR`/`CHRS`, `chars(i)` is computed from
the face's text when the near-duplicate test needs it (only for faces
that reach a page, or `similar`).

**Shared strings.** The compiler may store each distinct string once in
`STRS` (`--strings dedupe`); spans then overlap. This needs no reader
support and is allowed in 3.0 files too.

A reader bound-checks all of these when it uses them: a packed slot or a
varint past the section's end ends the list, a code past its table reads
0.0, a start out of order gives an empty list. At open it checks only the
headers: `FNCB`'s starts ascending and within the section, `DNSP`/`ENGP`'s
`bits` in 1..=32 and their term count.

### 5.4 Ids across releases: `ALIA`, `RETD` (format 3.1)

A face's id is a hash of its text (§3), so a face whose stored text
changes gets a new id. `ALIA` maps the old id to the new one; `RETD`
lists ids removed on purpose. Both are optional: a reader without them
treats old ids as unknown.

- `ALIA` records `[old, new]` are sorted by old, with chains already
  followed to their end by the compiler. The target may be absent from a
  smaller set (`core`, `lite`), like any other id of a face not in it.
- `RETD` records `[id, reason]`: 1 pruned, 2 licence, 3 blocklisted,
  4 quality cut, 0 other.
- The reader resolves aliases wherever an id comes in: `get`,
  `SearchOptions::exclude`, usage (`UsageMap`; an old and a new id's
  weights add, capped at 1), the blocklist and overlays.
  `Database::id_status(id)` says `Live`, `Aliased(new)`, `Retired(reason)`
  or `Unknown`. It follows at most 8 hops, against a damaged file's cycles.
- The compiler refuses an old id that is a face of the set, a cycle, two
  targets for one id, or an id both renamed and retired.
- Source: `data/aliases.jsonl` (`--aliases FILE`), one JSON object a line:

  ```
  {"old": "k0123456789ab", "new": "kba9876543210", "why": "dialogue stripped"}
  {"old": "k00000000beef", "retired": "licence"}
  ```

  `retired` is one of `pruned`, `licence`, `blocklisted`, `quality`,
  `other`. Blank lines and `#` comments are skipped; any other unreadable
  line is an error. Nothing checks yet that every id of the previous
  release is either still present or listed here.

## 6. Versioning

- A new **optional** section, or a new manifest key, is a **minor** bump.
- A new **required** section or any change to a record layout is a
  **major** bump; older readers refuse the file with `Incompatible`.
  The one exception is 3.1's compact encodings: new required sections that
  are alternatives to 3.0 tables, so a 3.0 reader refuses such a file
  (`Incompatible`, an unknown required tag) and a 3.1 reader reads both.
- A writer records the lowest minor that describes the file: `3.0` when it
  uses only 3.0 sections (`--encoding legacy`, no aliases), else `3.1`.
- The data release is `data_version` (`X.Y`) and changes with every data
  release; `content_hash` identifies a file exactly.

## 7. Building

```
emoticond-compile [--repo DIR] [--engine DIR] [--data DIR] [--situations FILE] [--aliases FILE]
                  [--set full|core|lite] [--select FILE] [--neighbours K] [--policy private|public]
                  [--encoding legacy|compact|small] [--data-version X.Y] [--build-date D]
                  [--source-rev R] [-o FILE] [--if-stale] [--verify]
```

The compiler needs an engine export (`meta.json`, `faces.jsonl`,
`vocab.jsonl`, `dense.bin`, `engine.bin`; `--engine`, default
`<repo>/work/engine`). The export comes from the data pipeline, which is
not part of this repository: released data files are published in
[emoticond-data](https://github.com/Cloveian/emoticond-data) and installed
with `emoticond data fetch`.

- `--set core|lite` keeps the faces in a selection list (`--select FILE`,
  sorted FaceIds one per line; default `<engine>/../sets/<policy>/core.txt`).
  Neighbour slots of dropped faces become `NONE` in place (ranks kept),
  canonical picks of dropped faces fall away, boosts stay (they are keyed
  by id). `lite` also cuts every neighbour list to 32 (`--neighbours K` for
  any set). A set compiled this way is byte for byte the file compiled from
  an export already filtered to the same faces.
- `--policy public` checks the export's provenance before compiling:
  `meta.json` must say `policy: public`; every face word must carry `wc`
  labels and every vocab key `ev` evidence (docs/public-data.md), and every
  label must be a class the policy allows. Otherwise it exits 1 and lists
  each class with its count. The policy is fixed in the compiler
  (`emoticond_compile::policy`), never read from the export.
- `--encoding`: `compact` (the default; every lossless saving of §5.3),
  `legacy` (format 3.0 exactly as the first compiler wrote it, byte for
  byte) or `small` (compact with `FNQ1`). One table at a time:
  `--nums f32|q16|q8`, `--lists u32|packed`, `--postings u32|varint`,
  `--chars stored|derived`, `--strings plain|dedupe`.
- `--data-version` defaults to `dev`; `--build-date` and `--source-rev` are
  recorded as given (`source_rev` defaults to a digest of the inputs).
- `-o` defaults to `<repo>/work/data/<set>.kmj`. `--if-stale` does nothing
  when the output is newer than every input.

The inputs are read through an adapter (`emoticond_compile::legacy`): the
engine export, `data/canonical_hand.jsonl`, `data/canonical.jsonl`,
`data/lexicon_phrases.jsonl`, `data/boosts.jsonl`, `data/grammar/*.json`,
`data/licence/{LICENCE,ATTRIBUTION}.txt` and `data/situations.json`, all
into `emoticond_compile::Sources`. `emoticond` compiles `<engine dir>/full.kmj`
itself when it is missing or older than any input.

The committed fixture `engine/crates/emoticond/tests/fixtures/tiny.kmj` is
built from sources in `emoticond-compile/tests/tiny.rs`; rebuild it after a
deliberate change with `EMOTICOND_BLESS_FIXTURE=1 cargo test -p
emoticond-compile --test tiny`.
