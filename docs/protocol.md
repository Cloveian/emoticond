# emoticond protocol v1 (stdio NDJSON)

For front-end authors. This is the protocol as implemented by `emoticond serve`
(engine/crates/emoticond-cli/src/proto.rs). Background is in
api-frontends.md; where the two differ, this file describes what is built.

## Starting it

```
emoticond serve [--frontend NAME] [--idle SECS] [--profile NAME] [--config PATH] [--no-config] [--set KEY=VALUE]...
```

- One process per client. Start it when the UI opens. It exits with status
  0 at EOF on stdin, on `quit`, or after `--idle` seconds without a request
  (default `[daemon] idle_exit` = 120; `0` = never).
- `--frontend quickshell` selects `[profile.quickshell]` in config.toml
  (default profile: `serve`). Option flags such as `--safety moderate` work
  here as they do in the CLI.
- Requests are read from stdin and responses written to stdout, one JSON
  object per line (UTF-8, `\n`). Nothing else goes to stdout. Logs go to
  stderr.
- Data: `$EMOTICOND_DATA_FILE` (a `.kmj`, opened as is); else, when a
  development export is present (`$EMOTICOND_ENGINE`, default
  `$EMOTICOND_REPO/work/engine`), `full.kmj` beside it, compiled from the
  export when it is missing or stale (docs/format.md); else the data search
  path: `$EMOTICOND_DATA`, the config's `data.dirs`, then
  `$XDG_DATA_HOME/emoticond`, each `$XDG_DATA_DIRS/emoticond` and
  `<exe>/../share/emoticond`. The first dir holding a data file wins; in it,
  the config's `data.dataset` (`core` by default) goes before the other set.
  `emoticond data fetch` installs a release into `$XDG_DATA_HOME/emoticond`.

## Ready line

The first line is always the ready line. `data` is null and
`counts.emoticon` is 0 when no data was found; `ms` is how long opening the
data took.

```json
{"ready":true,"protocol":1,"version":"1.0.0",
 "data":{"path":"…/.local/share/emoticond/core.kmj","version":"1.0","dataset":"core","format":"3.1","faces":69668},
 "counts":{"emoticon":69668},
 "features":["opts","ids","fields","explain","browse","similar","complete","get","pick","report",
             "block","usage","set_defaults","supersede","cancel","info","consent"],
 "disclaimer":{"version":2,"short":"Reports are saved on this computer until you choose whether to send them.",
               "text":"Reporting helps improve emoticond search. …"},
 "sending":false,"sending_off_by":"unasked","popularity":"local","profile":"quickshell","warnings":[],"ms":1}
```

- `version` is the code version (`X.Y.Z`); `data.version` is the data
  release (`X.Y`, or `dev` for a development build).
- `disclaimer.text` is the full text, to show once (see "Reports").
  `disclaimer.short` is the footer line for both report menus. Never write
  your own wording.
- `sending` says whether reports are sent. When it is false,
  `sending_off_by` says why: `unasked` (the user hasn't answered "send
  reports?" yet; nothing is sent until they do), `user` (they said no, or
  set `feedback.send = false`) or `policy` (the packager turned it off).
  `disclaimer.short` already matches; show it as is.
- **Asking.** While `sending_off_by` is `unasked`, ask once in your UI,
  with no default (a yes and a no, nothing preselected), and send the
  answer with `consent` (below). The answer is saved for every front-end
  and the CLI, so a user is only ever asked once.
- `popularity` is `off`, `local` (the default) or `shared`.
- `warnings` lists config problems found at start: `{level, code, key, message}`.
- Check `features` before using an optional op.

## Requests and responses

Every request has an `"op"`; a line without one gets a `bad_request`
error.

- `id` (integer or string) is optional. Every response to a request that has
  an `id` echoes it.
- Responses come in request order, one per request, with these exceptions:
  `cancel` never gets a response, and `pick`, `report`, `block`, `unblock`
  and `usage` get one only if the request has an `id`.
- `ok` is `true` or `false` on every v1 response.
- Unknown top-level keys and unknown `opts` keys are ignored, and the
  response carries a warning: `"warnings":[{"level":"warning","code":"unknown_key","key":"foo","message":"…"}]`.
  An unknown `op` is an error.

Errors:

```json
{"id":24,"ok":false,"error":{"code":"invalid_option","key":"opts.limit","message":"invalid value for `search.limit`: expected an integer, got a string"}}
{"id":null,"ok":false,"error":{"code":"bad_json","message":"expected value at line 1 column 1"}}
```

Error codes: `bad_json`, `bad_request`, `unknown_op`, `invalid_option`,
`not_found`, `not_allowed` (`consent` when the config or the policy decides),
`unsupported` (no data loaded) and `internal`. The daemon never
exits because of a bad request.

## Options (`opts`)

`opts` takes the library's `SearchOptions` in snake_case, with dotted names
as nested objects: `{"limit":30,"safety":"moderate","styles":{"lenny":"hide"},"emotions":{"target":{"sad":0.3}},"explain":"reading"}`.
The full list and the defaults are in options.md §3.1. `emoticond config show`
prints the effective values.

Layers, lowest first:

1. built-in defaults (strict safety, crude faces hidden, limit 40)
2. config.toml, then its `[profile.<frontend>]`
3. the request layer: `set_defaults`, then the top-level `limit` and
   `explain` (`true` means `"reading"`), then `opts`
4. environment variables and `emoticond serve` flags
5. the policy (`/etc/emoticond/policy.toml`): locks and ceilings, always last

The daemon then adds two things itself:

- **usage**: local popularity from `usage.json`, unless popularity is `off`,
  the request sends `opts.usage`, or the client pushed a map with `usage`.
- **blocklist**: every blocked face goes into `exclude`. That covers the
  state blocklist (offensive reports), the user's
  `~/.config/emoticond/overlays/blocklist.txt`, the system blocklist, and
  faces this process just blocked.

`offset`, `seed`, `exclude` and `usage` pass through unchanged.

## Ops

### `search`

```json
→ {"op":"search","id":12,"q":"really mad","chan":"main","opts":{"limit":3,"explain":"reading"},"fields":["flags","emotions"]}
← {"id":12,"ok":true,"q":"really mad","mode":"search","corrected":null,"term_key":"mad",
   "reading":{"line":"very mad (angry)","mode":{"kind":"terms"},
              "terms":[{"term":"mad","emotions":["angry"],"modifier":"high","negated":false}]},
   "n":3,"counts":{"emoticon":3},
   "results":[{"id":"k767d91d00c95","text":"((ヾ(≧皿≦ﾒ)ﾉ))","kind":"emoticon","score":1.6897,"rank":0,
               "flags":[],"emotions":{"angry":0.985,"laughing":0.048,"scared":0.051}}, …],
   "ms":7.7}
```

- Every result has `id`, `text`, `kind`, `score` and `rank`. The `id` is the
  stable face id (`k` + 12 hex digits). Use it as the row key and in `pick`,
  `report` and `exclude`. A `score` can only be compared within one
  response.
- `fields` adds extra data per result: `flags` (`suggestive`, `explicit`,
  `lenny`, `crude`, `multi`, `not_face`, `pinned`, `long`), `emotions` (the
  top 3), `emotions_all`, `quality` (1–8), `attrs`, `canonical_for`, and
  `why` (needs `explain:"full"`).
- `reading` is present when `explain` is `reading` or `full`. Show
  `reading.line` under the search box. `reading.mode.kind` is `concept`,
  `terms`, `situation`, `sentence`, `partial` (with `completions`), `glyph`,
  `tags`, `nothing` or `empty`.
- `corrected` is set when an unknown word was read as another word (a
  misspelling, a stretched word, or a typed emoticon such as `:/` read as
  its concept); in a longer query it names the last correction made.
- `clamped` lists the options the policy changed. It is absent when nothing
  was changed.
- There are no counts of hidden results: a filtered face is simply absent.
- **Empty `q`** returns the browse list (`"mode":"browse"`): the canonical
  starter faces first, then your history. Each result carries
  `"from":"canonical"` or `"from":"history"`.
- The library is kaomoji only: every result has `"kind":"emoticon"`. A
  front-end that wants emoji keeps its own list and mixes it in.
- `chan`: see "Supersession".

### `browse`

`{"op":"browse","id":22,"opts":{"limit":20},"section":"all"}` gives the same
response as an empty-query `search`. `section` can be `starter` (canonical
faces only), `recent` (history only; `popular` is the same until shared
popularity exists) or `all`.

### `explain`

```json
→ {"op":"explain","id":18,"q":"a bit sad"}
← {"id":18,"ok":true,"q":"a bit sad","reading":{"line":"a bit sad (sad)","mode":{"kind":"terms"},"terms":[…]}}
```

`"full":true` adds `reading.debug`, the parser's dump. Its format is
unstable.

### `similar`

`{"op":"similar","id":21,"face":"k2026da3e4989","opts":{"limit":20}}` gives
a search-shaped response with `"mode":"similar"`. `face` is an id or the
exact face text. The request's own `id` is the request id.

### `complete`

```json
→ {"op":"complete","id":19,"prefix":"emba","limit":3}
← {"id":19,"ok":true,"prefix":"emba","completions":[{"text":"embarrassed","source":"term","pinned":true}, …]}
```

### `get`

`{"op":"get","id":20,"ids":["k2026da3e4989"]}` (or `"texts":[…]`) returns
`{"entries":[{"id","text","quality","flags","attrs","emotions":{every emotion},"canonical_for":[…]}]}`.
An unknown face gives `null` in its slot.

### `pick`

```json
→ {"op":"pick","id":15,"about":12,"face":"k897607ff96f1","rank":1}
← {"id":15,"ok":true,"recorded":true,"logged":false}
```

Send `pick` when the user activates a face.

- `about` is the `id` of the search the face came from. The daemon keeps the
  last 32 search responses. Without `about`, send `q`; the daemon then
  searches it again to find the concept.
- `face` is an id or the exact text. `rank` is optional; the default is the
  face's position in that search.
- `recorded`: local popularity was updated. It is false when popularity is
  `off`.
- `logged`: the pick was written to the development pick log
  (`dev.pick_log`), which is off unless configured.

### `report`

There is one message for both menus. Every click sends one report.

| Menu | `reason` | Needs | Local effect, at once |
|---|---|---|---|
| query | `read_well` | `about` or `q` | none |
| query | `read_wrong` | `about` or `q` | none |
| query | `missing` | `about` or `q` | none |
| face | `great_fit` | `face` | counts as a pick (popularity) |
| face | `other_word` | `face` | none |
| face | `no_fit` | `face` | demotes the face for this concept (`overlays/boosts.jsonl`) |
| face | `offensive` | `face` | **blocklisted and hidden at once, everywhere, always** |
| either | `note` | `note` text | none |
| either | `clear` | `clears` (the reason withdrawn; optional), `face` for a face report | withdraws your earlier choice in that slot on the same query and face; undoes `offensive` (unblocks) and `no_fit`. Without `clears`, the newest pending choice for that query and face |

Reports replace each other only within a **slot**: on a query, the verdict
(`read_well` / `read_wrong`), `missing` and `note` are separate slots; on a
face, the fit (`great_fit` / `other_word` / `no_fit`), `offensive` and `note`
are. So "interpreted well" and "didn't return what I wanted" both stand, and
a face can be both `no_fit` and `offensive`. Un-choosing a menu item sends
`clear` with `clears` set to that item's reason.

```json
→ {"op":"report","id":40,"about":12,"reason":"offensive","face":"k767d91d00c95"}
← {"id":40,"ok":true,"queued":true,"sent":false,"sending":true,
   "footer":"Reports are sent with your search, its reading and the top 20 faces shown.",
   "effects":["blocked"],"blocked":["k767d91d00c95"],"report_id":"1ca060a666e53a74","key":"7dba712510c031e9"}
→ {"op":"report","id":41,"about":12,"reason":"read_wrong","note":"I meant mad as in crazy"}
→ {"op":"report","id":42,"about":12,"reason":"clear","clears":"no_fit","face":"k767d91d00c95"}
← {"id":42,"ok":true,…,"effects":["unblocked"],"unblocked":["k767d91d00c95"],…}
```

- `note` is optional with every reason, and required for `note`. It may be
  at most `feedback.custom_max_chars` characters (500).
- **Context.** With `about`, the daemon takes the query, the reading, the
  concept and the top 20 faces shown from that search. Without it (for
  example after an idle restart), send `q` and optionally `reading`,
  `term_key` and `shown` (a list of ids). Whatever is missing, the daemon
  fills in by searching `q` again with the request's `opts`. A robust client
  sends `about` and `q`.
- **What is stored.** Every report goes to
  `~/.local/state/emoticond/reports/queue.jsonl`. A report holds the query,
  the reading, the concept, the menu choice, the face and its rank, the top
  20 shown ids, the note, safety and styles, the engine and data versions,
  the disclaimer version, the time and a random id. A later report with the
  same `key` (same query and target) supersedes the earlier one.
- **Sending.** `sent` is always false: reports are queued and the daemon
  sends them in the background once they are 2 minutes old
  (docs/collector.md), so one undone within that time is never sent.
  `sending` tells you whether they will be sent. `sending_off_by` (`unasked`, `user` or `policy`) is present
  when sending is off. Show `footer` under the menu.
- **Effects.** `effects` lists what was applied: `blocked`, `unblocked`,
  `picked`, `demoted` or `undemoted`. `skipped` lists effects that were not
  applied, because `feedback.apply_locally = false` or popularity is off.
  The offensive block always applies.
- After `offensive`, remove the row right away and show *Hidden. Undo*.
  Undo sends `clear` for the same face, or `unblock`.
- A demote is written to the state overlay (`overlays/boosts.jsonl`) and
  applies from the next search.
- Disclaimer: store `feedback.disclaimer_seen` in your own settings. When
  the ready line's `disclaimer.version` is higher, show `disclaimer.text`
  once before the first report.
- A face reason without `face`, an unknown `reason`, or a `note` without
  text is a `bad_request`. An unknown `face` is `not_found`.

### `consent`

The user's answer to "send reports?", asked in your UI while the ready
line says `"sending_off_by":"unasked"`.

```
→ {"op":"consent","id":"c","send":true}
← {"id":"c","ok":true,"sending":true,"footer":"Reports are sent with your search, its reading and the top 20 faces shown."}
```

- Saved in `~/.local/state/emoticond/consent.json` for every front-end and
  the CLI (`emoticond reports on|off` changes it later).
- Yes sends only reports made from then on, never ones already queued.
- `not_allowed` when `feedback.send` is set in a config file, the
  environment or a flag (that wins), or the policy turned sending off.

### `block` / `unblock`

```json
→ {"op":"block","id":16,"face":"k2026da3e4989"}
← {"id":16,"ok":true,"face":"k2026da3e4989","changed":true}
```

These are the explicit forms. They write the state blocklist, which every
front-end reads. `changed` is false when there was nothing to do.

### `set_defaults`

`{"op":"set_defaults","id":0,"opts":{"limit":50,"explain":"reading"}}`
answers `{"id":0,"ok":true}`. The opts are checked as a request's would be
(a bad value is an `invalid_option` error), then used under every later
request's `opts`. Each call replaces the previous one. Send your settings
once instead of on every keystroke.

### `usage`

`{"op":"usage","id":23,"map":{"global":{"k…":0.5},"by_term":{"idk":{"k…":0.8}}}}`
is for clients that keep their own popularity store. From then on that map
is used instead of `usage.json`. `"map":null` goes back to the store.

### `cancel`

`{"op":"cancel","target":12}` drops request 12 if it has not started. The
dropped request is answered `{"id":12,"ok":false,"cancelled":true}`. Only
read-only ops (`search`, `browse`, `similar`, `explain`, `complete`, `get`,
`info`) can be cancelled. `cancel` itself gets no response.

### `info`

`{"op":"info","id":1}` returns the ready line's keys plus `defaults` (the
effective `SearchOptions`), `session_defaults`, `config_files`, `state`
(the paths of `usage`, `blocklist`, `queue` and `boosts`) and `dev` (the dev
log paths, or null).

### `quit`

`{"op":"quit"}`: the process exits with status 0.

## Supersession

The daemon reads every line already sent before it starts the next request.
A `search` or `browse` with `"chan":"main"` (any string) is answered at
once with `{"id":N,"ok":false,"superseded":true}` when a later search on the
same channel is already queued. You still get exactly one response per
request, in order. Requests without `chan` are never superseded. Show only
the newest response.

## State shared between processes

The picker's daemon, the CLI and any other front-end share state through
files under `$XDG_STATE_HOME/emoticond/`: `usage.json`, `blocklist.txt`,
`reports/queue.jsonl` and `overlays/boosts.jsonl`. Each process re-checks
the files at most once a second, before a search. A face reported offensive
in one process is gone from every other process within a second, and at
once from the process that took the report.
