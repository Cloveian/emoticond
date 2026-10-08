# emoticond

a (legitimately) clever kaomoji search engine ฅ^•ﻌ•^ฅ

```
$ emoticond search "idk"              ¯\_(ツ)_/¯   ╮(╯_╰)╭   ┐( ˘_˘ )┌ …
$ emoticond search "not without you"  (っ•ᴥ•)(•ᴗ•⊂)❤   (つ・▽・)つ⊂(・▽・⊂) …
$ emoticond search "running late"     ε=ε=┏(0-0)┛   !!(((((っ;ﾟ∀ﾟ)っ …
```

- **a rust library, a cli, and a little daemon** for picker front-ends
- **~176,000 kaomoji**, each scored on 19 emotions and tagged with what it shows
- **reads queries the way people actually type them:** phrases (`it's fine`),
  intensity (`a bit sad`), mixtures (`shy proud`), slang and typed emoticons
  (`eep`, `:/`), typos, half-typed words
- **fast:** a few ms per search, ~1 ms to start, and the data is memory-mapped
  so every process shares one copy

---

- [install](#install)
- [command line](#command-line)
- [library](#library)
- [for picker front-ends](#for-picker-front-ends)
- [options and safety](#options-and-safety)
- [your data](#your-data)
- [data sets](#data-sets)
- [how a search works](#how-a-search-works)
- [docs](#docs)
- [on LLM use](#on-llm-use)
- [licence](#licence)

---

## install

| | |
|---|---|
| arch | `makepkg -si` in `packaging/aur/emoticond-bin` and `packaging/aur/emoticond-data` (or `emoticond` to build from source) |
| homebrew | `brew install Cloveian/emoticond/emoticond` |
| cargo | `cargo binstall emoticond-cli`, or `cargo install emoticond-cli` to build (rust 1.89+) |
| anything else | a binary from [releases](https://github.com/Cloveian/emoticond/releases) (linux x86_64/aarch64, macos apple silicon), on your PATH |

> not on the AUR yet: after the recent AUR security incidents, new account
> registration is paused, so i can't make an account to publish there. the
> PKGBUILDs are ready, so `makepkg` gets you the same thing for now

then get the data (~13 MB) into `~/.local/share/emoticond/` (not needed
with the `emoticond-data` package):

```sh
emoticond data fetch                  # or --set full / lite, or a version like 1.0
```

every version lives in [emoticond-data](https://github.com/Cloveian/emoticond-data),
one file per set, so you can also just download one. `emoticond info` tells you
what it found and where it looked.

## command line

```sh
emoticond search shy proud                # faces, one per line
emoticond search -n 5 --format json sad   # also tsv, jsonl, dmenu, alfred
emoticond explain "a little sad"          # how it read the query
emoticond browse                          # starter faces, then your recent picks
emoticond similar "(╥﹏╥)"                 # faces like this one
emoticond menu                            # pick with fuzzel/rofi/walker/wofi/tofi/bemenu/dmenu
emoticond config show                     # every setting and where it came from
emoticond stats on                        # share anonymous usage stats (asked once otherwise)
```

- `emoticond menu` is made for a keybind (hyprland:
  `bind = SUPER, period, exec, emoticond menu`): ask, show, copy, done
- exit codes: 0 found, 1 no results, 2 bad usage, 3 no data

## library

```toml
[dependencies]
emoticond = "1"
```

```rust
use emoticond::{Database, OpenOptions, Safety, SearchOptions};

let db = Database::open(OpenOptions::file("core.kmj"))?;

let mut opts = SearchOptions::default(); // strict safety, 40 results
opts.limit = 10;
opts.safety = Safety::Moderate;

for hit in db.search("shy proud", &opts).hits {
    println!("{}  {}", hit.text, hit.id); // d(^//∇//^)b  k6ef56ed18cc6
}
```

- runnable: `cargo run -p emoticond --example search -- core.kmj "shy proud"`
- **the core is pure:** no env vars, config, clock or printing; everything
  comes in through `OpenOptions` and `SearchOptions`
- **stable ids:** `FaceId` is `k` + 12 hex of the SHA-1 of the face's text
- also: `browse`, `similar`, `complete`, `get`, `read` (how a query was
  read), live overlays and blocklists
- companion crates: `emoticond-config` (config, profiles, policy, paths),
  `emoticond-state` (popularity, reports, blocklists), `emoticond-compile`
  (builds `.kmj` files)

## for picker front-ends

`emoticond serve` speaks newline-delimited JSON over stdin/stdout:

```
→ {"op":"search","id":7,"q":"really mad","chan":"main","opts":{"limit":30}}
← {"id":7,"ok":true,"q":"really mad","results":[{"id":"k…","text":"(╬ಠ益ಠ)","score":1.93,"rank":0}, …],"ms":4.1}
```

- **made for search-as-you-type:** stale searches on a channel get dropped
- **ops:** search, browse, explain, similar, complete, pick, report,
  block/unblock, set_defaults
- **starts when you need it**, exits after `--idle` seconds (default 120)
- full spec: docs/protocol.md. a quickshell service is in `integrations/quickshell/`

## options and safety

set per query, in `~/.config/emoticond/config.toml` (with per-front-end
profiles), or as flags (`--safety moderate`)

| option | default | |
|---|---|---|
| `safety` | `strict` | `strict`, `moderate` (lenny is ok), `off` |
| `styles` | crude hidden | `lenny`, `crude`, `long`: allow / demote / hide |
| `limit` | 40 | |
| `figures` | `auto` | `single`, `pair` (hugs, high fives), `any` |
| `emotions`, `min_quality`, `max_len` | — | filters |

- `/etc/emoticond/policy.toml` can lock settings (like forcing `strict`)
- strict means strict: no query word unlocks hidden faces
- everything else: docs/options.md

## your data

kept in `~/.local/state/emoticond/`, shared by every front-end

- **popularity:** faces you pick move up. stays on your computer
- **reports** (from the picker's menus) apply right away on your machine:
  offensive faces disappear, bad fits drop
- **reports are sent in** (that's what reporting is) with the query, how
  it was read and the top 20 faces, and nothing else
  - undo within 2 minutes and it never gets sent
  - `emoticond reports off` keeps them on your computer
- **usage stats are opt-in:** you're asked once ("share usage stats? [y/n]",
  no default). say yes and, once a day, a picker sends which faces got
  picked for which built-in search words (counts rounded into ranges, never
  what you typed), no id. nothing is shared until you say yes;
  `emoticond stats on|off` changes it (docs/collector.md)
- **blocking:** `emoticond block FACE` / `unblock FACE`
- **overlays:** your own picks, phrases and boosts in `~/.config/emoticond/overlays/`

## data sets

| set | faces | size | |
|---|---|---|---|
| `core` | ~70,000 | ~13 MB | default: every face that can reach a first page |
| `full` | ~176,000 | ~24 MB | everything |
| `lite` | ~70,000 | ~11 MB | shorter neighbour lists |

each one is a single memory-mapped `.kmj` file (docs/format.md)

**versions:** data is `X.Y`, emoticond is `X.Y.Z`. same X = they work
together; Y = the data release that emoticond was made for; Z = fixes that
don't touch the data

## how a search works

all the heavy stuff (models, embeddings, tagging) happens when the data is
built. at search time:

1. **read the query** into emotions: 12,907 terms and 3,487 phrases, plus
   intensity, negation, typed emoticons, typo fixing and completion
2. **score every face** on emotion match, retrieval rank, tags, one face vs
   two, quality, and safety/style penalties. plain concepts (`idk`) put
   their hand-picked faces first
3. **rank** it and drop near-duplicates (nobody needs twelve copies of the same bear)

pasting part of a face (`ツ`, `¯\_`) searches face text instead.
`emoticond explain --full QUERY` shows every step

## docs

| | |
|---|---|
| docs/protocol.md | the `serve` protocol |
| docs/options.md | options, config, profiles, policy |
| docs/api-frontends.md | library API and front-end design |
| docs/format.md | the `.kmj` format |
| docs/collector.md | what reports send, and where |
| docs/public-data.md | what the public data contains |
| docs/development.md | building and testing |

## on LLM use

this started as a hehe funny side project ( ˶ˆ꒳ˆ˵ ) that i figured i might
as well make available for other people to use.

- **LLMs did the volume:** most of the code, the emotion ratings, the tags,
  the phrase list and judging search results
- **i directed it and quality-checked it,** and made some of the base data:
  hand ratings, tags and search judgments, a few hours here and there, that
  the models were trained and checked against
- **the kaomoji themselves aren't AI-generated** (as far as i know): they're
  collected from public kaomoji lists

## licence

- **code:** MIT or Apache-2.0, your pick
- **data** (the `.kmj` files): CC BY 4.0. the faces come from public kaomoji
  lists; docs/public-data.md lists the sources and their attribution
