# Public data

What a public build of the data may contain, and how that is enforced.
The files in [emoticond-data](https://github.com/Cloveian/emoticond-data)
(`core`, `full`, `lite`) are all public builds. They are made by the
project's data pipeline, which is not part of this repository; this page
describes the policy that pipeline applies and that `emoticond-compile
--policy public` enforces.

## Licences

| What | Licence |
|---|---|
| Code (everything under `engine/`, `integrations/`) | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| The project's own data: emotion and attribute scores, quality ratings, tags written by the project's labelling agents and models, search phrases, situations, canonical picks, boosts, grammar word lists, Japanese glosses | CC BY 4.0 (`LICENSE-DATA`, summary in `data/licence/LICENCE.txt`) |
| Face strings | collected from the sources in `data/licence/ATTRIBUTION.txt`, each under its own terms |

Credit for the data: "emoticond data, CC BY 4.0", with a link to the licence
and a note of any changes.

## Faces

Every face ships, from every source. Faces are short functional strings in
wide public circulation. A face's scores (emotions, quality, crude,
suggestive) are always the project's own model output, never a source's
labels.

## Words on faces

Each word attached to a face carries a provenance class (`wc` in the
export, `WCLS` in the data file):

| Class | Origin | Public |
|---|---|---|
| `a` | the project's face-tagging agent | yes |
| `m` | the project's tag model (CNN) | yes |
| `l` | the project's tag language model | yes |
| `o` | the project's own corpus labels (actions, subject, description) | yes |
| `s:kmoji`, `s:kaomojiru`, `s:fontvibe` | scraped tags from sources that allow redistribution | yes |
| `s:fontvibe/kaosute` | fontvibe tags on faces from its `kaosute` pool | no |
| `s:emojicombos` | single-word emojicombos user tags (no whitespace) | yes |
| `s:emojicombos/multi` | multi-word emojicombos tags | no |
| `s:ekohrt` | ekohrt/emoticon_kaomoji_dataset tags | no |
| `s:kaomojikuma` | kaomojikuma headings | no |
| `s:gsozai` | gsozai categories (its category tree is not mirrored) | no |

In a public build a word from a disallowed class is never added. The
compiler refuses a public build that contains one, writes
`policy=public@<digest>` into the manifest and sets header flag bit 0
(docs/format.md).

## Search keys

A search key (a vocabulary term with an emotion profile and neighbour
lists) ships when at least one piece of its evidence is allowed: the
project's own data (emotion names, the hand-built lexicon, agent and model
tag vocabularies, own corpus, phrases, situations, canonical terms, the
English words of our Japanese glosses), a shippable scraped tag (rules as
above), Unicode CLDR / emojilib names, or the reduced Japanese set below.

A key that exists only because a non-shippable source used that text is
dropped, even though its profile and neighbour lists are derived data:
without the key nothing reaches them, and the set of such keys is that
source's curation.

## The reduced Japanese set

Japanese keys that come from IME dictionary readings ship only in reduced
form, never as a whole dictionary and never as a reading-to-face mapping.
A Japanese key with no other shippable evidence ships only if:

- the project's own English gloss of it names a key of the hand-built
  lexicon;
- it is not a name;
- it is written only in Japanese script and is at most 8 characters long;
- it is among the 1,000 most used such terms.

## Derived data

Neighbour lists (a fine-tuned e5 retriever and the affect engine), vocab
profiles and the face scores are derived data. They may be computed from
every source that permits derived use. kaomojis.jp (face strings CC0; its
terms forbid using its labels or ordering for a similar service) contributes
face strings only: none of its labels, order or membership is a feature of
any model or ranking.

## Data sets

| Set | Faces | Size | Contents |
|---|---|---|---|
| `full` | 176,126 | 24.3 MB | every face |
| `core` | 69,668 | 12.8 MB | every face that reaches a first page (40) for any shipped key or test query under three option sets, plus every canonical and boosted face |
| `lite` | 69,668 | 11.1 MB | `core` with both neighbour lists cut to 32 |

(data 1.0)

## Checks

Before a public build is released, the pipeline re-derives from its raw
inputs what each face may carry and which keys may exist, and fails on any
word or key without an allowed origin. The repository itself is checked the
same way: no file in it carries a non-shippable source's per-face tags or a
non-shippable key list.
