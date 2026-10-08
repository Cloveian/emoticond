# Options and settings

What can be configured in emoticond, at which layer, under which name and
with which default.

Companion docs:
- **api-frontends.md** covers the Rust API shape, the CLI, bindings and
  launcher integrations.
- **protocol.md** is the daemon protocol as built.
- **format.md** is the data file (`.kmj`).

This doc names options and says what they mean. Where an option meets a
wire format or an API type, the exact encoding is in those docs.

**Names**: the crate, binary, data and state dirs and the `EMOTICOND_*` env
prefix are all `emoticond`.

---

## 0. Principles

1. **The library is pure.** It reads no env vars or config files and has no
   clock or RNG of its own. Everything reaches it as `OpenOptions` (once, when
   the database is opened) or `SearchOptions` (per query). Front-ends map
   their own config, env and CLI flags onto those two structs. The shared
   config crate (section 7) does that mapping for front-ends that want it.
2. **Defaults are for strangers.** `SearchOptions::default()` is the
   fresh-install experience: safe and unsurprising. A personal setup lives
   in a config file, not in the built-in defaults.
3. **Options carry intent. Weights stay internal.** An option should say what
   the user wants ("no lenny faces", "pairs", "only short faces"), never how
   the ranking gets there ("lenny penalty 0.15"). Ranking constants are tuned
   together against nDCG, so exposing one freezes it as API. They stay
   internal, with an unstable escape hatch for tuning (section 3.4).
4. **The query text and the options are separate channels, and the options
   win on safety.** A query word like `lewd` or `lenny` can *ask*; the
   options decide whether asking is *allowed*.
5. **Policy clamps, it doesn't replace.** An admin can set a ceiling (for
   example "never looser than strict") or lock a value. The user can still
   choose anything inside the ceiling.

---

## 1. The layered model

| Layer | Who sets it | When | How it reaches the engine | Examples |
|---|---|---|---|---|
| **(a) Data / build** | the maintainer (or a packager rebuilding data) | the build that makes the data set | baked into the shipped file; read-only at runtime | which set (`core` / `full` / `lite`), quality cut, safety thresholds |
| **(b) Open** | the front-end (from config + policy) | `Database::open(OpenOptions)` | `OpenOptions` struct | data file or dirs, data set, overlays, blocklist, policy, default `SearchOptions` |
| **(c) Query** | the front-end, per request (from user prefs + UI state) | every `search()` | `SearchOptions` struct (daemon: `"opts"` object; CLI: flags) | limit, safety, styles, figures, filters, usage map, seed, explain |
| **(d) User preferences** | the end user | settings UI or a hand-edited file | config file, loaded by the shared config crate, mapped onto (b) and (c) | `safety = "moderate"`, popularity mode |
| **(e) Packager / admin policy** | distro packager, sysadmin | install time | `/etc/emoticond/policy.toml`, read by the config crate; clamps (b)–(d); also passed to the library as `OpenOptions.policy` so a raw query can't escape it | disable report sending, safety ceiling, popularity ceiling, locked keys |

Data flow:

```
policy.toml ─┐
             ├─> config crate ──> OpenOptions ────────> Database::open
system cfg ──┤    (merge, clamp,    (incl. policy +
user cfg ────┤     validate)         default SearchOptions)
env / CLI ───┘                    └> per-request SearchOptions ─> db.search(q, opts)
                                     (front-end fills usage map,     (library clamps again
                                      seed, UI toggles)               against OpenOptions.policy)
```

The policy is enforced twice: in the config crate, so settings UIs can grey
out locked controls, and in the library, so that a front-end which skips the
config crate (a third-party binding, a hand-written daemon request) still
can't loosen safety. Enforcing it in the library doesn't break purity
because the policy arrives as an open-time argument.

---

## 2. What is not an option

### 2.1 Ranking constants (`emo.rs`)

These are internal. Where a user has a reason to care, an intent-level
option reaches the behaviour instead.

| Constant | Value | Reached through |
|---|---|---|
| `DEFAULT_WEIGHTS` emotion/dense/engine/lexical | 0.5 / 1.2 / 1.2 / 0.2 | `advanced.tuning.weights` (unstable, 3.4) |
| structured-query reweighting (emotion ×2, dense/engine ×0.4) | | internal |
| `W_ADVERB` 0.4, `W_CUTE` 0.2, `MIN_PRESENT` 0.08 | | internal |
| `LOW_TARGET` 0.35 | | `intensity = "low"` (3.1) |
| `W_QUALITY` 0.06 (soft quality term) | | the hard cut is `min_quality` |
| `W_BOOST` 0.08 × curated boost | | boosts are data (overlayable) |
| `CANON_BONUS` [30, 20, 10] | | on/off is `pinned`; it only has to exceed every other score |
| near-duplicate Jaccard 0.55 | | `dedupe = "off" / "normal" / "strong"` → 1.0 / 0.55 / 0.40 |
| grammar word lists (intensifiers, negators, stopwords, lewd words, single/pair words, typed emoticons) | | data: `data/grammar/<lang>.json`, compiled into the data file |
| pair/single preference (−0.35/+0.05, ±0.3, 0.15·pref, −0.08) | | direction via `figures` |
| not-a-face penalty 0.4·(1−face) | | plus the hard filter `faces_only` |
| length penalty 0.012/char over `long_at`, max 0.2 | | `styles.long`, `long_at`, hard cut `max_len` |
| suggestive 0.05·4^(x−1), ×2 bonus when asked | | `safety` (3.2) |
| lenny −0.15 / +0.4 when asked | | `styles.lenny` |
| crude −0.1 unless asked | | `styles.crude` |
| `high` intensity bonus 0.15 | | `intensity = "high"` |
| completion blend (4 completions, −0.25/rank) | | on/off is `complete_partial` |
| usage lift 0 / 0.06 / 0.15 / 0.3 | | `usage_weight` |
| ranking head `limit*20` | | internal; accounts for `offset` |
| ties by face id | | always on (3.3) |

### 2.2 Glyph and typed-emoticon search

Pasted symbol text (`ツ`, `¯\_`) does a substring search over faces. Typed
emoticons that the grammar lists (`:/`, `T_T`) are read as their concept
first. The only switch is `glyph_search = "off"`; the scoring is internal.

### 2.3 Environment variables

| Variable | Sets | Used by |
|---|---|---|
| `EMOTICOND_CONFIG` | config file path, or `none` | config crate |
| `EMOTICOND_PROFILE` | the `[profile.<name>]` to apply | config crate |
| `EMOTICOND_DATA` | dirs prepended to `data_dirs` | config crate |
| `EMOTICOND_IDLE_EXIT` | `daemon.idle_exit` | config crate |
| `EMOTICOND_REPORT_ENDPOINT` | `feedback.endpoint` (`none`: no sending) | config crate |
| `EMOTICOND_PICK_LOG` | `dev.pick_log` (`off`: none) | config crate |
| `EMOTICOND_TUNING_WEIGHTS` | `advanced.tuning.weights` (`a,b,c,d`) | config crate |
| `EMOTICOND_DATA_FILE` | a `.kmj` to open as is | `emoticond` binary |
| `EMOTICOND_REPO`, `EMOTICOND_ENGINE` | a development export to compile from | `emoticond` binary |
| `EMOTICOND_DATA_URL` | mirror for `emoticond data fetch` | `emoticond` binary |
| `EMOTICOND_LAUNCHER` | launcher for `emoticond menu` | `emoticond` binary |

The `emoticond` binary finds its data file as protocol.md ("Starting it")
describes.

### 2.4 Data build parameters

Set when a data file is compiled (format.md §7), recorded in its manifest,
never runtime options:

| Parameter | Shipped value |
|---|---|
| quality cut | faces predicted ≤ 3 (on 1–8) are left out of every set |
| set | `core` (faces that can reach a first page, plus every canonical and boosted face), `full` (everything), `lite` (core with shorter neighbour lists) |
| safety thresholds | `safety.innuendo_at` 0.5, `safety.sexual_at` 1.5, `crude.at` 0.5 |
| multi-line art | pruned from every set |

### 2.5 Taste encoded in data, not code

| Behaviour | Where | Treatment |
|---|---|---|
| canonical "the one" picks | `data/canonical.jsonl`, `data/canonical_hand.jsonl` | shipped data. The user adds or overrides with an overlay (6.1); `pinned=false` turns it off per query |
| per-term boosts | `data/boosts.jsonl` | shipped data, overlayable |
| phrases | `data/lexicon_phrases.jsonl` | shipped data, overlayable (user phrases) |
| situations | `data/situations.json` | shipped data |

### 2.6 Front-end settings (`[ui]`, `[daemon]`)

These are read by front-ends, not the library:

| Key | Default | Meaning |
|---|---|---|
| `ui.show_reading` | false | true sets `explain = "reading"` unless `explain` is set explicitly |
| `ui.max_results` | unset | most results a front-end lists |
| `ui.start_timeout_ms` | 5000 | how long to wait for the helper process to start |
| `daemon.idle_exit` | 120 | `emoticond serve` exits after this many idle seconds; 0 = never |

---

## 3. Per-query options (`SearchOptions`)

All names are snake_case and identical across the Rust struct, the config
file (under `[search]`), the daemon's `"opts"` object and (kebab-cased) the
CLI. Defaults are the **stranger** defaults.

### 3.1 Option table

| Option | Type | Default | Semantics |
|---|---|---|---|
| `limit` | u16 (1–500) | 40 | Results to return, after dedupe and filters. |
| `offset` | u32 | 0 | Skip this many results, counted after dedupe and filters. Paging is stable only with the same query, options and `seed`. Cost is the same as a fresh query (a few ms), so there are no cursors. |
| `safety` | `strict` \| `moderate` \| `off` | `strict` | Suggestive content handling; see 3.2. Clamped by policy. |
| `styles.lenny` | `hide` \| `demote` \| `allow` | `demote` | `( ͡° ͜ʖ ͡°)` family (lenny ≥ 0.5). A query that asks (`lenny`, or a phrase with the lenny attribute) lifts `demote` and `allow` to a bonus; `hide` beats asking. |
| `styles.crude` | `hide` \| `demote` \| `allow` | `hide` | Faces with the crude flag (rude gestures, vulgar text). Asking with a lewd word doesn't unlock `hide`. |
| `styles.long` | `hide` \| `demote` \| `allow` | `demote` | Faces over `long_at` characters. `demote` is the length penalty; `hide` drops them. |
| `long_at` | u16 | 14 | The threshold `styles.long` uses. |
| `max_len` | u16 | unset (none) | Hard cap in characters. Overrides `styles.long` for longer faces. Useful for status bars and chat inputs. |
| `faces_only` | bool | false | Drop entries the model says aren't faces (face < 0.5: dividers, sparkle rows). The soft not-a-face penalty applies either way. |
| `figures` | `auto` \| `single` \| `pair` \| `any` | `auto` | `auto`: query words and terms decide, plain emotions lean single. `single`/`pair` act as if the query said `solo`/`together`. `any` turns the figure preference off. |
| `intensity` | `auto` \| `low` \| `high` | `auto` | Applies to the whole query, as if prefixed `a bit` / `very`. Query modifiers still apply per term. |
| `emotions.target` | map emotion → f32 0–1 | empty | Adds a target-profile term: the face's normalised intensities should be *near* these values (the `a bit` mechanism, per emotion). `{sad = 0.3}` means "mildly sad". Works with an empty query (browse mode). |
| `emotions.min` | map emotion → f32 0–1 | empty | Hard filter: the face's score for that emotion is ≥ the value. |
| `emotions.max` | map emotion → f32 0–1 | empty | Hard filter: ≤ the value. `{angry = 0.2}` means "never angry". |
| `min_quality` | f32 1–8 | unset | Hard cut on predicted quality, on top of the build cut (≤ 3 is already gone). |
| `dedupe` | `off` \| `normal` \| `strong` | `normal` | Near-duplicate thinning: Jaccard thresholds 1.0 / 0.55 / 0.40 over character sets. |
| `pinned` | bool | true | Canonical faces lead single-concept queries (`idk`, `shrug`). Covers shipped picks and user overlay picks. |
| `usage` | `UsageMap` (section 4) | empty | Caller-supplied, already-decayed personal popularity. The library never reads usage from disk. |
| `usage_weight` | `off` \| `low` \| `normal` \| `high` | `normal` | How much `usage` lifts a face. `high` can beat a curated boost but never a pinned canonical face. Config key: `popularity.weight`. |
| `variety` | f32 0–1 | 0 | Seeded jitter on non-pinned scores, so repeat queries can differ. 0 is fully deterministic. |
| `seed` | u64 | 0 | RNG seed for `variety`. Front-ends pick it, for example per launcher session (stable while you type, new next time) or per day. |
| `exclude` | list of face ids | empty | Hide these for this query: already-inserted faces, "show me others". |
| `lang` | `auto` \| `en` \| `ja` | `auto` | Query language. One index holds English and Japanese keys (`嬉しい` parses as a happy term). `en` and `ja` are accepted and currently behave as `auto`. |
| `correct` | bool | true | Stemming, un-stretching (`noooo` → `no`, `happpy` → `happy`) and spelling correction of unknown words: a lone word within 1–3 edits, a word of five letters or more inside a longer query within one edit (`tentitive proude`). A misspelling the phrase lexicon lists under a well-known word reads as that word (`tenitive` → `tentative`, with its pins). The result reports `corrected`. |
| `complete_partial` | bool | true | A partial word blends its likeliest completions: alone (`embar`) or after words that were read (`very sa`, `im so ti`, `it was just o`). Search-as-you-type wants true; scripting usually wants false. |
| `glyph_search` | `auto` \| `off` | `auto` | Pasted symbol text (`ツ`, `¯\_`) does a substring search over faces. Typed emoticons the grammar lists (`:/`, `:(`, `T_T`, `xD`, `<3`; `emoticons` in `data/grammar/<lang>.json`) are read as the concept they stand for first, with or without a trailing `!`/`?`, and punctuation after a word (`ok!`, `huh?`) never makes a glyph search. `off` always parses as words. |
| `explain` | `off` \| `reading` \| `full` | `off` | `reading`: the one-line "how I read it" (`very angry (angry) · pair · pinned faces first`). `full`: also the parser's dump and a per-result score breakdown, for debugging. Unstable format. |
| `tuning` | `Tuning` (3.4) | unset | Unstable escape hatch. Not for front-ends. |

### 3.2 Safety levels

The data has `suggestive` on 0–3 (model-predicted, continuous): about 1 is
innuendo (the lenny face itself is 1), about 2 is clearly sexual, about 3 is
explicit (body-part strings). In the full data set about 1,000 faces are
≥ 0.5, about 300 are ≥ 1.5 and about 100 are ≥ 2. There's also a separate
crude flag.

A query **asks** when it contains a lewd word (`lewd`, `nsfw`, `horny`,
`suggestive`, from the grammar data) or a phrase with the `suggestive`
attribute. Ambiguous words don't ask: `butt` can return body-part strings,
and under `strict` it won't.

| | suggestive < 0.5 | 0.5 – 1.5 (innuendo) | ≥ 1.5 (sexual, explicit) | when the query asks |
|---|---|---|---|---|
| **`strict`** (default) | normal | demoted | **removed** | nothing changes; the reading notes "suggestive (off: strict)" |
| **`moderate`** | normal | demoted | demoted (on a curve) | bonus instead of the penalty |
| **`off`** | normal | normal | normal | bonus |

The thresholds come from the data file's manifest (format.md §4.1).

Why the cutoff is 1.5 and not 0.5: a cutoff at 0.5 would hide the lenny face
and many winks. Innuendo is the `styles.lenny` question, not a safety one.

Why asking doesn't unlock anything under `strict`: strict means strict.
Someone who wants to see suggestive faces changes the setting to
`moderate` or `off`.

`styles.crude` is separate from safety because crude means rude gestures and
vulgar text, not sexual content. It defaults to `hide` for strangers. Its
coverage is incomplete (not every `╭∩╮` face is flagged).

Filtered results are simply absent: responses carry no count of what was
hidden.

### 3.3 Things that are deliberately not options

| Not an option | Why |
|---|---|
| Score weights, penalties, bonus magnitudes | Tuned jointly against the judged queries; any change needs re-evaluation. Exposing them would freeze today's numbers as API. Intent-level options (styles, figures, intensity, safety) cover every reason a user had to want them. |
| Grammar word lists (intensifiers, negators, stopwords, lewd words) | Language data. They ship in the data set per language and can be extended with a phrases overlay, not per query. |
| Deterministic tie-breaking | Always on: ties are broken by face id. Non-determinism comes only from `variety` + `seed`, which are explicit. Tests, paging and caching depend on this. |
| A `min_score` cutoff | Scores have no stable scale: a pinned face scores 30+, the long tail about 1. Use `limit`, `min_quality` and the emotion filters. |
| The canonical bonus size | `pinned` covers the decision; the size only has to win. |
| Timeouts or budgets | A query costs a few ms over the full set. |
| Raw query rewriting hooks | Front-ends can rewrite the string before calling. |

### 3.4 The tuning escape hatch

```toml
[advanced.tuning]            # UNSTABLE: may change or vanish in any release
weights = { emotion = 0.5, dense = 1.2, engine = 1.2, lexical = 0.2 }
```

- `SearchOptions.tuning` sits behind a cargo feature (`unstable-tuning`) and
  is ignored, with a warning, when the feature is off. Release builds of
  front-ends leave it off.
- The `EMOTICOND_TUNING_WEIGHTS` env var (`a,b,c,d`) sets it too.
- New tuning keys get added only when an evaluation needs them, not just
  because a constant exists.

---

## 4. Popularity

One setting, three levels, plus a small number of supporting keys.

| `popularity.mode` | Effect | Stored | Sent |
|---|---|---|---|
| `off` | no usage map; nothing recorded | nothing (existing history kept until "clear") | nothing |
| `local` (**default**) | the front-end records picks, builds a decayed `UsageMap` and passes it per query | `$XDG_STATE_HOME/emoticond/usage.json` | nothing |
| `shared` | as `local`, plus anonymous usage stats (§4.3) | as local, plus `$XDG_STATE_HOME/emoticond/outbox/` | once a day: bucketed pick counts and an "in use" upload (collector.md) |

`local` is the default: it never leaves the machine, it's what makes the
picker learn "your" shrug, and it matches the "recently used" behaviour of
every emoji picker. A settings page shows it with a **Clear history**
button.

### 4.1 The library side

The library takes a `UsageMap` and does no IO:

```
UsageMap {
  global:  { face_id -> f32 0..1 }                 # how much you use this face at all
  by_term: { term_key -> { face_id -> f32 0..1 } } # what you picked for this concept
}
```

- `term_key` is the **parsed concept** (the vocab term, phrase key or
  situation name that `pinned` would use), not the raw query, so `idk`,
  `i dunno` and `idk lol` share history.
- `by_term` counts 4× `global`: a face's weight for a query is
  (global + 4 × by_term) / 5. Picking `¯\_(ツ)_/¯` for `idk` mostly helps
  `idk`.
- The library exports pure helpers in `emoticond::usage`: `record(state,
  pick, now)`, `decay(state, now, half_life_days)` and `to_map(state)`.
  They take `now` (unix ms) as an argument, so the library stays clockless
  and every front-end computes decay the same way. `emoticond-state`
  persists the state.

### 4.2 Popularity settings

| Key | Type | Default | Notes |
|---|---|---|---|
| `popularity.mode` | `off` \| `local` \| `shared` | `local` | policy can cap it (`popularity.max`). Unset, the user's answer to "share usage stats?" (asked once, `emoticond stats on\|off`) makes it `shared` or keeps `local` |
| `popularity.half_life_days` | f32 1–3650 | 30 | exponential decay of each pick's weight |
| `popularity.max_entries` | u32 | 5000 | (term, face) pairs kept; the lowest decayed weight is evicted first |
| `popularity.store` | path | `$XDG_STATE_HOME/emoticond/usage.json` | aggregated decayed scores plus a last-updated time; **no raw query text, no timestamps per pick** |
| `popularity.remember_terms` | bool | true | false keeps only `global` (no record of what you searched for) |
| `popularity.weight` | `off` \| `low` \| `normal` \| `high` | `normal` | maps to `SearchOptions.usage_weight` |
| `popularity.share_interval_days` | u16 | 7 | how often `shared` batches are queued |
| `popularity.share_endpoint` | URL | `https://emoticond.mewo.gay/v1/stats` | where usage stats go; policy may set it |

### 4.3 What `shared` records

The aim is per-concept "which face do people pick", the same thing
`canonical.jsonl` captures by hand, with nothing that identifies a person.

- Per batch: a list of `(term_key, face_id, bucket)`. `bucket` is the pick
  count in that period, rounded into 1, 2–4, 5–19 or 20+.
- `term_key` comes only from shipped vocab or phrase keys. Personal overlay
  phrases and free text are **never** included.
- Also included: data-set version, engine version and the batch period.
  There's no install id, user id, locale or timezone. Every batch gets a
  fresh random token so retries can be deduplicated.
- A server receiving these must drop the source IP and keep only aggregates
  across at least k installs before using a term.

Batches are sent with the daily upload (collector.md, "Usage stats").
Shared counts come back to users as **data updates** (new boosts and
canonical candidates in the next data release), never as a live query-time
service.

### 4.4 Development log

`dev.pick_log` (env `EMOTICOND_PICK_LOG`; `off` turns it off) appends every
pick as JSONL: query, chosen face, rank and the full shown list. That is
evaluation data, richer and more personal than popularity, so it has its
own switch, is **off by default**, and is never sent anywhere.

---

## 5. Feedback and reports

Two menus:
- **query menu**: interpreted well / interpreted incorrectly / didn't return
  what I wanted / custom note
- **face menu**: really good fit / fits but not the word I used / doesn't
  fit / offensive or explicit / custom note

### 5.1 Behaviour

1. Clicking an item **always creates a report**. The disclaimer is shown
   in the menu itself (a footer line, plus the full text the first time).
2. The report goes into a local queue. The daemon's background sender
   delivers it once it is 2 minutes old, unless sending is off
   (collector.md).
3. **Local effects happen immediately**, whether or not anything is sent:
   - *offensive or explicit*: the face is added to the user's blocklist
     (`$XDG_STATE_HOME/emoticond/blocklist.txt`) and hidden at once.
   - *really good fit*: counts as a pick for popularity (if mode ≠ off).
   - *doesn't fit*: demotes that face for that `term_key` in the state
     overlay (a negative boost), so the same bad answer stops coming back.
   - *fits but not the word I used*: kept as a report only; no local effect.
   - Local effects can be turned off with `feedback.apply_locally = false`,
     except the offensive hide, which is always applied.

### 5.2 Feedback settings

| Key | Type | Default | Notes |
|---|---|---|---|
| `feedback.menus` | bool | true | show the menus at all; a front-end may hide them |
| `feedback.send` | bool | true | set here, it decides; unset, `emoticond reports on\|off` does (on by default). The policy can always turn it off |
| `feedback.endpoint` | URL | `https://emoticond.mewo.gay/v1/reports` | policy may override, for example an org's own collector |
| `feedback.queue` | path | `$XDG_STATE_HOME/emoticond/reports/queue.jsonl` | |
| `feedback.queue_max` | u32 | 500 | the oldest is dropped beyond this |
| `feedback.queue_max_age_days` | u16 | 180 | |
| `feedback.apply_locally` | bool | true | 5.1 step 3 |
| `feedback.custom_max_chars` | u16 | 500 | hard cap on free text |

Both the user (`emoticond reports off`, or `feedback.send = false`) and
the packager (a policy lock) can turn sending off. Reports are still queued
and their local effects still apply; the menu footer then reads "saved on
this computer only".

### 5.3 Report contents

The report holds the raw query text (the user typed it into a menu that says
it will be sent), the `reading`, the parsed concept, the menu choice, the
face id and text if there is one, the result rank, the top 20 faces shown,
the note, the `safety` level and styles in force, data and engine versions,
the disclaimer version, the time and a random report id.

Not included: usage history, other queries, install id, locale, timezone,
hostname.

### 5.4 Disclaimer versioning

- The disclaimer text ships with the library, with an integer
  `disclaimer_version`.
- Each report records the version that was shown.
- A front-end stores `feedback.disclaimer_seen = <version>` in its own
  settings. When the shipped version is higher (which happens when report
  contents change), the full text is shown again on the next menu open
  before a report is created.

### 5.5 Privacy defaults, summarised

| Data | Leaves the machine by default? |
|---|---|
| keystrokes, partial queries | never recorded |
| picks | stored locally (aggregated) under `local`; under `shared` (opt-in), queued as bucketed per-concept counts |
| reports | sent when the user clicks a report item (unless sending is off) |
| dev pick log | off; never sent |

---

## 6. Open-time options (`OpenOptions`)

| Option | Type | Default | Notes |
|---|---|---|---|
| `file` | path | unset | a data file to open; when set, `data_dirs` and `dataset` are not consulted |
| `data_dirs` | list of paths | (config crate) `$XDG_DATA_HOME/emoticond`, each `$XDG_DATA_DIRS/emoticond`, `<exe>/../share/emoticond` | the first dir holding a data file wins. The library takes the list as given |
| `dataset` | `core` \| `full` \| `auto` | `core` | which of `core.kmj` / `full.kmj` a dir is searched for first; `auto` is full if installed, else core |
| `overlays` | list of `OverlaySource` | none (config crate: the state and user overlay dirs, 6.1) | applied in order, after shipped data |
| `blocklist` | set of face ids | empty (config crate: user blocklist plus `/etc/emoticond/blocklist.txt`) | never returned, whatever the options |
| `defaults` | `SearchOptions` | `SearchOptions::default()` | what `Database::defaults()` hands out; how a front-end applies the user's config once |
| `policy` | `Policy` | none | ceilings and locks (section 7.3) that the library enforces on every query |

`set_blocklist`, `reload_overlays` and `set_defaults` change these without
reopening the data file (api-frontends.md).

### 6.1 Overlays

These are user files layered over the shipped data, in the same formats as
the shipped files (so `canonical_hand.jsonl` works as an overlay too).

| File (in `$XDG_CONFIG_HOME/emoticond/overlays/`) | Effect |
|---|---|
| `canonical.jsonl` | `{"term","text"\|"id","rank"}`. User picks go **before** shipped ones for that term; `{"term","clear":true}` drops the shipped picks |
| `phrases.jsonl` | new phrases or spellings (`{"key","match":[...],"p":{...}}` or `{"key","alias_of":"shrug"}`) |
| `boosts.jsonl` | `{"term","id","boost":-3..3}`; negative values demote |
| `blocklist.txt` | one face id or face text per line |

Machine-written overlays (from feedback, 5.1) go to
`$XDG_STATE_HOME/emoticond/overlays/`. Hand-written ones in the config dir
win over them.

Details (the core's `emoticond::overlay` module is normative):

- **Sources and order.** `OpenOptions.overlays` / `Database::reload_overlays`
  take `OverlaySource`s: a directory (any of the four files above), a single
  file (kind from its name; `canonical_hand.jsonl` counts as canonical), inline
  text with a file name, or a typed `Overlay` built in code (no `fs` needed).
  Each source is a layer; later layers win. `emoticond-config` lists the state
  dir, then the config dir, then `data.overlays`.
- **Ids** may be written `k2026da3e4989` or bare `2026da3e4989`; rows may
  name a face by `text` instead. Rows naming faces this data set lacks, and
  bad lines, are warnings (`overlay_bad_line`, `overlay_unknown_face`, ...),
  never errors.
- **Pins:** per term, a layer's pins (by rank) go before everything below
  it; at most three lead. `clear` drops the shipped picks and earlier layers'
  pins. Hits pinned by the user carry `Flags::USER_PIN`; the reading says
  "your pinned faces first".
- **Phrases** also accept `words` and `attr` (`cute`, `lenny`,
  `suggestive`) like the shipped lexicon; `alias_of` may name a user phrase,
  a shipped phrase or a vocab term. User spellings beat shipped ones and a
  vocab word of the same length. The reading adds "yours" and
  `ReadTerm::user`; `SearchResult::term_source` is `overlay`.
- **Boosts** apply to the query's concept (`term_key`), the overlay winning
  per (term, face) over shipped boosts. A negative boost also unpins a
  shipped canonical face for that term (so "doesn't fit" can move it), never
  a user pin.
- **`blocklist.txt`** in an overlay dir hides faces like
  `OpenOptions.blocklist`; `set_blocklist` replaces only the latter.

### 6.2 User pins versus usage

A face pinned in the user's own `canonical.jsonl` leads that term
regardless of `usage`. Usage can reorder everything below the pins but never
beats a pin. The user's explicit statement beats inferred habit.

---

## 7. Config files, precedence and policy

### 7.1 Locations (XDG)

| What | Path |
|---|---|
| user config | `$XDG_CONFIG_HOME/emoticond/config.toml` (`~/.config/emoticond/config.toml`) |
| system config | `$XDG_CONFIG_DIRS/emoticond/config.toml` (`/etc/xdg/emoticond/config.toml`) |
| policy | `/etc/emoticond/policy.toml` (a fixed path; it doesn't follow XDG, so users can't redirect it) |
| user overlays | `$XDG_CONFIG_HOME/emoticond/overlays/` |
| data | `$XDG_DATA_HOME/emoticond/` (user installs, `emoticond data fetch`), `/usr/share/emoticond/` (packages) |
| cache | `$XDG_CACHE_HOME/emoticond/` |
| state (usage, reports queue, machine overlays, blocklist) | `$XDG_STATE_HOME/emoticond/` |

On macOS the config, data and state live under
`~/Library/Application Support/emoticond`; on Windows under
`%APPDATA%\emoticond` (config) and `%LOCALAPPDATA%\emoticond`.

### 7.2 Precedence

```
policy locks  >  CLI flag  >  env var  >  per-request opts (daemon)  >  [profile.<name>]  >  user config  >  system config  >  built-in defaults
                                                                                                                                (stranger defaults)
policy ceilings clamp the final result (e.g. safety never looser than policy's minimum)
```

- **Profiles.** A front-end passes its name (`cli`, `serve`, `quickshell`,
  `menu`). `[profile.<name>]` tables override the top-level ones for that
  front-end only. One file can then say "the CLI is `safety = off`, the
  launcher is `strict`". `--profile` and `EMOTICOND_PROFILE` pick one
  explicitly.
- **Env vars** are a short documented list (2.3), not a generic mapping.
- **Per-request daemon opts** override the config the daemon loaded but are
  still clamped by policy.
- `emoticond config show` prints every effective value and where it came
  from.

### 7.3 Policy semantics

The policy has two kinds of entries:

- **ceilings**: an ordered setting that can't be looser than X (`safety.min`,
  `popularity.max`). Users choose freely inside the range.
- **locks**: a fixed value for any config key (`feedback.send = false`). The
  settings UI shows the control disabled with "Set by your system
  administrator".

`[endpoints]` may set `feedback.endpoint` and `popularity.share_endpoint`.
Per-query options that would cross a ceiling are clamped, not rejected.
The response lists `clamped: ["safety"]`.

### 7.4 Sample `config.toml`

A user who wants suggestive faces demoted rather than hidden, crude faces
shown lower rather than hidden, and the reading line in their picker:

```toml
# ~/.config/emoticond/config.toml
config_version = 1

[search]
limit           = 50
safety          = "moderate"          # demote suggestive faces instead of hiding them
pinned          = true
dedupe          = "normal"
lang            = "auto"

[search.styles]
lenny = "demote"
crude = "demote"
long  = "demote"

[popularity]
mode           = "local"
half_life_days = 60
remember_terms = true

[feedback]
send          = true
apply_locally = true

[ui]
show_reading = true       # explain = "reading"
max_results  = 100

[daemon]
idle_exit = 120           # seconds; 0 = never

[profile.cli.search]
complete_partial = false
explain = "off"

[advanced.tuning]         # unstable; needs a build with the unstable-tuning feature
# weights = { emotion = 0.5, dense = 1.2, engine = 1.2, lexical = 0.2 }
```

### 7.5 Sample `policy.toml`

```toml
# /etc/emoticond/policy.toml -- read by every front-end via the config crate,
# and handed to the library as OpenOptions.policy.
policy_version = 1

[ceilings]
safety.min     = "strict"     # users may not choose moderate/off
popularity.max = "local"      # no shared counts from this machine

[locks]
feedback.send       = false   # kill switch: reports are queued locally, never sent
styles.crude        = "hide"

[endpoints]
# feedback.endpoint = "https://feedback.example.org/emoticond"   # org collector instead

[data]
blocklist = ["/etc/emoticond/blocklist.txt"]
dataset   = "core"
```

With `feedback.send` locked to false, the report menus stay. Local effects
(hide offensive, demote) still work, and the disclaimer footer reads "saved
on this computer only".

A distro that wants report sending off for every user of its package ships
this file. A package that ships no policy file leaves everything open.

---

## 8. What front-ends should show

The config crate's settings registry (`emoticond_config::settings()`) tags
each key with the page it belongs on.

### 8.1 Basic settings page (every launcher)

| Control | Maps to | Notes |
|---|---|---|
| **Content filter**: Strict / Moderate / Off | `safety` | one-line help per level; disabled above a policy ceiling |
| **Lenny faces**: Hide / Show less / Show | `styles.lenny` | may share one control with crude faces |
| **Crude faces**: Hide / Show less / Show | `styles.crude` | |
| **Learn from what I pick**: Off / On this device / Also share anonymous counts | `popularity.mode` | **Clear history** button next to it |

### 8.2 Advanced (collapsed section or "Open config file")

`styles.long` / `long_at` / `max_len`, `faces_only`, `figures`,
`min_quality`, `dedupe`, `pinned`, `variety`, `lang`,
`popularity.half_life_days`, `popularity.weight`, `feedback.send`,
`feedback.apply_locally`, `ui.show_reading`, and an "Open overlays folder"
button.

### 8.3 File only

Limits and offsets (front-ends own these), `remember_terms`, queue sizes,
data dirs and data set, profiles, the dev log, tuning.

### 8.4 Not settings at all (UI state)

Paging, the emotion sliders of a browse mode (these go into
`emotions.target` per request), and the seed (chosen per session by the
front-end).

---

## 9. Defaults at a glance

| Setting | Default | Notes |
|---|---|---|
| data set | `core` | `full` is the larger optional set |
| build quality cut | ≤ 3 dropped | a build parameter (2.4) |
| `safety` | `strict` | |
| `styles.lenny` | `demote` | |
| `styles.crude` | `hide` | |
| `styles.long` | `demote` (at 14) | |
| `figures` | `auto` | plain emotions lean single |
| `pinned` | true | shipped canonical and hand picks lead |
| `min_quality` | none | |
| `dedupe` | `normal` (0.55) | |
| `variety` | 0 | |
| `limit` | 40 | |
| `explain` | `off` | `ui.show_reading = true` gives `reading` |
| `popularity.mode` | `local` | |
| `feedback.send` | true | |
| usage stats (`popularity.mode = shared`) | asked once, nothing sent until yes | |
| dev pick log | off | |
| `daemon.idle_exit` | 120 | |
| policy | none | |

---

## 10. Validation and compatibility

### 10.1 Values

- **Out of range** (`limit = 0`, `min_quality = 11`): clamped to the nearest
  valid value, with a warning naming the key.
- **Unknown enum value** (`safety = "spicy"`): in a config file, the default
  for that key, with a warning. On `safety` the fallback is the stricter of
  the default and the policy floor, never looser.
- **Type error** (`limit = "ten"`): in a config file, the key is ignored and
  a warning is logged. In a daemon request or the CLI, the request fails
  with an error naming the key. A request is cheap to fix; a broken config
  shouldn't stop the picker from starting.
- **Contradictions** resolve by documented rule rather than error:
  `max_len` beats `styles.long`; `emotions.min` above `emotions.max` gives
  an empty result plus a warning; `variety > 0` with no `seed` uses seed 0.
- **Unknown emotion names** in `emotions.*` produce a warning and the entry is
  ignored. The emotion list comes from the data set's manifest, not the
  code.

### 10.2 Forward compatibility

- Unknown keys in config files, policy files and daemon `opts` are
  **ignored with a warning** (`unknown_key`). This lets one config work
  across front-end versions, and lets newer front-ends talk to an older
  daemon.
- **Exception: `policy.toml`.** An unknown key in `[locks]` or `[ceilings]`
  logs at error level, because an admin who believes something is locked
  must find out that it isn't. An unreadable policy fails closed (strict,
  no sending).
- `config_version` and `policy_version` are integers. A newer version than
  the reader understands means: read what it can, warn, and never loosen
  anything the older reader can't parse.
- `SearchOptions` and `OpenOptions` are `#[non_exhaustive]` with
  `Default`. Adding an option is never a breaking change. Serde uses
  `#[serde(default)]` and doesn't deny unknown fields, except in the CLI's
  `--strict-options` mode, where unknown keys are errors.
- A few aliases are accepted (`usage_weight` for `popularity.weight`,
  `tuning.weights` for `advanced.tuning.weights`); keys from older drafts
  are ignored with an `obsolete_key` warning.

### 10.3 In the daemon protocol and CLI

- Daemon: a request carries `"opts": { ...SearchOptions in snake_case... }`.
  The top-level `limit` and `explain` work too and map onto `opts`.
  Responses add `warnings` and `clamped` when they're non-empty.
  `set_defaults` saves a picker from resending its config with every
  keystroke. Details in protocol.md.
- CLI: one flag per option in kebab-case (`--limit 20`, `--safety moderate`,
  `--lenny hide`, `--figures pair`, `--min-quality 4.5`, `--max-len 12`,
  `--variety 0.3 --seed 7`, `--emotion sad=0.3`, `--explain reading`); any
  setting works as `--name VALUE`. Plus `--config PATH`, `--no-config`,
  `--profile NAME`, `--set KEY=VALUE` and `--opts JSON` for anything else.
- Results carry **stable face ids** (`k` + 12 hex digits). Usage maps,
  `exclude`, blocklists, overlays and reports all key on them.
