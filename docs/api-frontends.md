# Public surface: library API, CLI, protocol, front-ends

This doc covers everything a launcher, picker or app touches. Companion
docs:

- **options.md**: every per-query option (`SearchOptions`), open-time option
  (`OpenOptions`), user and packager setting, the policy layer, popularity
  and feedback behaviour, and their defaults.
- **protocol.md**: the stdio protocol of `emoticond serve`, as built.
- **format.md**: the data file.

This doc does not redefine options. Where an option crosses the API, the
wire or the CLI, this doc gives the **encoding** and names the option.

**Names**: the crates, the `emoticond` binary (daemon: `emoticond serve`),
data and state dirs and the `EMOTICOND_*` env prefix are all `emoticond`.

---

## 0. Summary

| Topic | Shape |
|---|---|
| Scope | Kaomoji only. Front-ends that also want emoji keep their own list and mix it in. |
| Library | One core Rust crate (`emoticond`) with a small synchronous API, `Database::open(OpenOptions)` and `db.search(text, &SearchOptions)`. Config (`emoticond-config`), state files (`emoticond-state`), the compiler (`emoticond-compile`) and the CLI/daemon (`emoticond-cli`) are separate crates over it. |
| Ids | **Every result carries a stable face id** (`k` + 12 hex digits), in every format. Usage maps, `exclude`, blocklists, overlays and reports all key on it. |
| Universal binding | The **CLI and the stdio NDJSON protocol**. Anything that can spawn a process can use the engine. Startup takes milliseconds, so spawning per query is fine. |
| CLI | One binary with subcommands. `emoticond menu` runs the whole dmenu-style flow (fuzzel, rofi, walker, wofi, tofi, bemenu, dmenu) from one keybind. Any setting works as a flag. |
| Protocol | NDJSON over stdio, one process per client (protocol.md). No socket daemon and no general D-Bus API. |

---

## 1. Rust library API

### 1.1 Principles

These follow options.md §0 and add API-level rules.

- **Synchronous, `&self`, no global state.** `Database` is `Send + Sync`
  and cheap to clone (an `Arc` over the memory map). A query takes a few
  ms, so there is no async API. Callers that want a thread can use one.
- **Pure.** No env vars, config files, clock, RNG, network or clipboard.
  Anything that changes results arrives as `OpenOptions` or `SearchOptions`.
  That includes usage, the blocklist, overlays, the seed, and `now` for usage
  decay.
- **Deterministic.** The same data, text and options give the same output,
  byte for byte. Ties break on `FaceId`. `variety`/`seed` is the only
  intended source of variation.
- **Safe defaults.** `SearchOptions::default()` is the stranger default from
  options.md §9.
- **Search never fails.** Any string is a valid query. Bad option values are
  clamped or defaulted with a **warning** (options.md §10.1), so the result
  carries `warnings` and `clamped`. It does not return a `Result`. Only the
  wire and CLI layers turn a *type* error into a request error.

### 1.2 Core types and signatures

Simplified; rustdoc on the `emoticond` crate is normative.

```rust
// ---- opening (OpenOptions: options.md §6) --------------------------------
#[derive(Clone)]
pub struct Database { /* Arc<Inner>; blocklist/overlays/defaults swapped atomically */ }

impl Database {
    pub fn open(opts: OpenOptions) -> Result<Database, OpenError>;
    /// A data file already in memory (no `fs` needed).
    pub fn from_bytes(bytes: impl Into<DataBytes>, opts: OpenOptions) -> Result<Database, OpenError>;
    pub fn info(&self) -> DbInfo;   // versions, path, data set, counts, emotion names, licence
    pub fn verify(&self) -> Result<(), String>;   // every section checksum

    // Runtime mutation without reopening. Atomic swaps; searches in flight
    // finish on the old value.
    pub fn set_blocklist(&self, ids: BTreeSet<FaceId>);
    pub fn reload_overlays(&self, overlays: &[OverlaySource]) -> Vec<Warning>;
    pub fn set_defaults(&self, defaults: SearchOptions);
    pub fn defaults(&self) -> SearchOptions;

    // ---- search ----------------------------------------------------------
    pub fn search(&self, text: &str, opts: &SearchOptions) -> SearchResult;
    /// The empty-query list: canonical starter faces, then the caller's usage.
    pub fn browse(&self, opts: &SearchOptions) -> SearchResult;
    /// How a query is read (the reading line), without searching.
    pub fn read(&self, text: &str, opts: &SearchOptions) -> Reading;
    /// Typeahead over vocabulary terms and the user's overlay phrases.
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<Completion>;

    // ---- entries ---------------------------------------------------------
    pub fn get(&self, id: FaceId) -> Option<Entry>;
    pub fn find_text(&self, text: &str) -> Option<Entry>;
    pub fn similar(&self, id: FaceId, opts: &SearchOptions) -> SearchResult;
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_;
    pub fn id_status(&self, id: FaceId) -> IdStatus;   // Live, Aliased, Retired, Unknown (format.md §5.4)
}

// ---- ids -----------------------------------------------------------------
/// 48 bits of SHA-1 over the exact text. String form "k" + 12 hex
/// ("k2026da3e4989"); Display / FromStr round-trip.
pub struct FaceId(/* u64 */);

// ---- results -------------------------------------------------------------
pub struct SearchResult {
    pub hits: Vec<Hit>,
    pub corrected: Option<String>,       // options.md `correct`
    pub reading: Option<Reading>,        // explain = reading | full
    pub term_key: Option<String>,        // parsed concept; keys usage + reports
    pub term_source: TermSource,         // shipped data or the user's overlay
    pub clamped: Vec<Cow<'static, str>>, // option names clamped by policy
    pub warnings: Vec<Warning>,
    pub safety: Safety, pub styles: Styles,   // in force for this result
}

pub struct Hit {
    pub id: FaceId,
    pub text: String,
    pub score: f32,                      // comparable within one result only
    pub rank: u32,                       // position (after offset)
    pub flags: Flags,                    // so a UI can badge without a get()
    pub why: Option<HitWhy>,             // explain = full: per-component scores
}

// Flags: SUGGESTIVE, EXPLICIT, LENNY, CRUDE, MULTI (pair/group), NOT_FACE,
// PINNED (canonical for this query), LONG, USER_PIN.

pub struct Entry {
    pub id: FaceId, pub text: String,
    pub quality: f32,                    // predicted 1..8
    pub flags: Flags,
    pub attrs: Attrs,                    // multi, cute, intensity, suggestive, lenny, face
    pub emotions: Vec<(String, f32)>,    // names from the data manifest
    pub canonical_for: Vec<String>,      // concepts it is "the one" for
}

// ---- reading -------------------------------------------------------------
pub struct Reading {
    pub line: String,                    // "very angry (angry) · pair · pinned faces first"
    pub mode: ReadMode,
    pub terms: Vec<ReadTerm>,            // structured, for UIs that render chips
    pub debug: Option<String>,           // explain = full: the parser's dump, unstable
}
pub enum ReadMode { Empty, Concept, Terms, Situation, Sentence,
                    Partial { completions: Vec<String> }, Glyph, Tags, Nothing }

// ---- picks and usage (options.md §4.1) -----------------------------------
pub struct Pick { pub id: FaceId, pub term_key: Option<String> }
pub mod usage {
    pub fn record(state: &mut UsageState, pick: &Pick, now: u64);
    pub fn decay(state: &mut UsageState, now: u64, half_life_days: f32);
    pub fn to_map(state: &UsageState) -> UsageMap;     // -> SearchOptions.usage
}

// ---- feedback: one record type for both menus ---------------------------
pub struct Report {
    pub v: u16,                          // record schema version (1)
    pub report_id: String,               // random, caller-supplied (library has no RNG)
    pub key: String,                     // supersession: hash(query, target, slot)
    pub ts: u64,                         // caller-supplied unix ms
    pub target: Target,                  // Query | Face { id, text, rank }
    pub reason: Reason,
    pub clears: Option<Reason>,          // for Clear: the reason withdrawn
    pub note: Option<String>,            // <= feedback.custom_max_chars
    pub query: String,
    pub term_key: Option<String>,
    pub reading: Option<String>,
    pub corrected: Option<String>,
    pub safety: Safety, pub styles: Styles,
    pub shown: Vec<FaceId>,              // top 20 as displayed
    pub engine: EngineStamp,             // engine + data version
    pub disclaimer_version: u32,
}

pub enum Reason {
    ReadWell, ReadWrong, Missing,                // query menu
    GreatFit, OtherWord, NoFit, Offensive,       // face menu
    Note, Clear,                                 // either menu
}
impl Report {
    pub fn for_query(r: &SearchResult, q: &str, reason: Reason) -> ReportBuilder;
    pub fn for_face(r: &SearchResult, q: &str, hit: &Hit, reason: Reason) -> ReportBuilder;
}
impl ReportBuilder {
    pub fn note(self, s: impl Into<String>) -> Self;
    pub fn stamp(self, report_id: impl Into<String>, ts: u64, disclaimer_version: u32) -> Self;
    pub fn build(self) -> Result<Report, ReportError>;
    /// The local effects options.md §5.1 asks for, as data the caller
    /// applies: Block, RecordPick, Demote.
    pub fn local_effects(&self) -> Vec<LocalEffect>;
}
```

A later report with the same `key` supersedes the earlier one, and `Clear`
withdraws it, so the last line for a query and face is its current state.
Every click still creates a report (options.md §5.1).

### 1.3 Errors

All error types implement `std::error::Error` and are `#[non_exhaustive]`.

```rust
pub enum OpenError {
    NotFound { searched: Vec<PathBuf> },
    Incompatible { found: String, supported: &'static str },  // format or data version
    Corrupt { section: &'static str },
    BigEndian,
    Io(std::io::Error),
}
pub enum ReportError { ReasonNeedsFace, ReasonNeedsQuery, NoteRequired, NoteTooLong { max: usize },
                       NotStamped, KeyMismatch, TooManyShown { max: usize }, SendingLocked }
pub struct Warning { pub level: Level, pub code: Cow<'static, str>, pub key: Option<String>, pub message: String }
```

There is no `SearchError`.

### 1.4 What is not in the core

| Not in core | Where it is |
|---|---|
| Config files, env vars, profiles, policy loading, platform paths | `emoticond-config`. It produces `OpenOptions` and `SearchOptions`. |
| Usage store, blocklist file, report queue, machine overlays, file locking, dev log | `emoticond-state`. Paths from options.md §7.1: `$XDG_STATE_HOME/emoticond/{usage.json, blocklist.txt, reports/queue.jsonl, overlays/}`. |
| Sending reports | `emoticond-state`'s sender (feature `net`), run by `emoticond serve` (collector.md) |
| Clipboard, typing (`wl-copy`, `wtype`, `xdotool`) | the CLI (`emoticond menu`) and each front-end |
| Compiling a data file | `emoticond-compile` (format.md §7) |
| Rendering, fonts, widths | front-ends (§7) |

Cargo features of `emoticond`: `fs` (open from paths, overlay files),
`mmap` (memory-map the data file), both default; `unstable-tuning`
(options.md §3.4). Without `fs` only `from_bytes` and code-built overlays
are available.

---

## 2. CLI

### 2.1 Commands

```
emoticond search [QUERY...|-] [OPTIONS] [--format plain|dmenu|tsv|json|jsonl|alfred]
emoticond explain QUERY [--json] [--full]
emoticond get ID|TEXT... [--json]
emoticond similar ID|TEXT [--format F]
emoticond complete PREFIX [-n N]
emoticond browse [--recent|--starter] [--format F]
emoticond pick ID|TEXT [--query Q] [--rank N]
emoticond report --reason R [--face ID|TEXT] [--query Q] [--note TEXT] [--json]
emoticond block ID|TEXT        emoticond unblock ID|TEXT
emoticond menu [--launcher L] [--action copy|type|print] [OPTIONS]
emoticond serve [--idle SECS] [--frontend NAME]      # protocol.md
emoticond config [show]        emoticond info [--json]
emoticond data fetch [X.Y] [--set core|full|lite]
```

**Option flags** follow options.md §10.3: any setting works as
`--name VALUE` (dots or dashes alike), with shortcuts such as `-n`/`--limit`,
`--safety`, `--lenny`/`--crude`/`--long`, `--figures`, `--min-quality`,
`--max-len`, `--faces-only`, `--no-pinned`, `--no-correct`,
`--emotion sad=0.3`, `--emotion-min`, `--emotion-max`, `--variety`,
`--seed`, `--offset`, `--exclude ID,ID` and `--explain reading`. Plus
`--config`, `--no-config`, `--profile`, `--set KEY=VALUE`, `--opts JSON`,
`--strict-options` and `--fields` (json/jsonl output).

- `QUERY...` words are joined with spaces. `-` reads the query from stdin.
- An empty query means `browse`, so `emoticond search ""` shows something
  useful.
- Warnings (unknown options, clamping) go to stderr, one line each. They
  never go to stdout, so pipes stay clean.

### 2.2 Output formats

| `--format` | Shortcut | Line shape | For |
|---|---|---|---|
| `plain` | (default) | `face` (the reading and any correction go to stderr) | humans, `head -1` |
| `dmenu` | `--dmenu` | `face\thint\tid` | rofi/fuzzel/wofi/tofi/bemenu/dmenu/walker `--dmenu` |
| `tsv` | | `id\temoticon\tface\tscore\t` | awk/cut scripts |
| `json` | `--json` | one object, same shape as a protocol v1 search response | programs |
| `jsonl` | | one hit per line | streaming consumers |
| `alfred` | | Alfred Script Filter `{"items":[...]}` (uid = face id) | Alfred; adaptable for Flow Launcher |

The `dmenu` hint is the concepts the face is canonical for, else its top
two emotions (`shrug · bored`). With fuzzel ≥ 1.11, show and return the
face with `--with-nth=1 --accept-nth=1`, or match on the hint too with
`--match-nth=1,2`. With plain dmenu, use `cut -f1`. The face is always the
first field; the id is the last, so a script can call `emoticond pick` with
an id instead of text.

### 2.3 `emoticond menu`: the easy path

dmenu-style launchers filter a fixed list. None of them can re-query a backend
on every keystroke without a plugin. `menu` uses a flow that works with all
of them:

1. **Ask.** The launcher opens with the browse list (canonical starter
   faces, then your history) on stdin, so Enter on a row picks that face at
   once. Typed text that matches no row is taken as a query.
2. **Search** with the user's config (`[profile.menu]`), and show the
   results in the same launcher in `dmenu` format.
3. **Act**: `copy` (`wl-copy`, or `xclip`/`xsel` on X11), `type` (`wtype`,
   `xdotool` or `ydotool`), or `print`. Then record the pick.

The launcher is `--launcher`, else `$EMOTICOND_LAUNCHER`, else the first of
fuzzel, rofi, walker, wofi, tofi, bemenu, dmenu that is installed. A name
not in the built-in table is run as a command line. A Hyprland bind is then
`bind = SUPER, period, exec, emoticond menu`. dmenu UIs have no context
menu, so reporting is done separately with `emoticond report`.

### 2.4 Finding data

`emoticond` opens, in order: `$EMOTICOND_DATA_FILE`; a development export's
`full.kmj` (protocol.md, "Starting it"); else the first `core.kmj` or
`full.kmj` in `$XDG_DATA_HOME/emoticond`, each `$XDG_DATA_DIRS/emoticond`,
then `<exe>/../share/emoticond`. `emoticond data fetch` downloads a release
from [emoticond-data](https://github.com/Cloveian/emoticond-data), checks
it against the release's `SHA256SUMS` and puts it in the first of these
dirs. Without a version it takes the newest data this build can read (data
`X.*` for code `X.Y.Z`). `emoticond info` prints the data path and set,
the code and data versions, the counts, the state paths and the config
files loaded.

### 2.5 Exit codes

| Code | Meaning |
|---|---|
| 0 | success: at least one result, or a command that has no results |
| 1 | no results (like grep, so `emoticond search x \|\| notify-send ...` works) |
| 2 | usage error: bad flag, option type error, or an unknown option under `--strict-options` |
| 3 | data not found or incompatible |
| 4 | state/I-O error, or a launcher/clipboard tool is missing (`menu`) |
| 5 | reserved: refused by policy (nothing is refused yet; reports are always queued) |
| 130 | `menu` cancelled (Esc in the launcher) |

---

## 3. Protocol v1 (stdio NDJSON)

The wire format is in protocol.md. The design choices behind it:

- **The process belongs to one client.** The client starts
  `emoticond serve` when the UI opens; `[daemon] idle_exit` (default 120 s)
  ends it. The data file is memory-mapped, so several clients share one
  page-cache copy.
- **No socket mode yet.** `serve --socket` is refused. With a fast start and
  shared memory, a shared daemon would mostly add lifecycle, staleness and
  permission problems. The one real gain, every client seeing the same
  blocklist and usage at once, comes from files the processes re-check
  before a search (protocol.md, "State shared between processes").
- **No general D-Bus API.** D-Bus fits only as a front-end adapter
  (KRunner, GNOME search), which is not built.
- **Per-query options** go in `opts`, exactly the `SearchOptions` of
  options.md §3.1 in snake_case. `set_defaults` lets a picker send its
  settings once instead of on every keystroke.
- **Supersession, not cancellation.** A query takes a few ms, so stopping
  one mid-flight is pointless. The problem is a backlog from fast typing;
  a `search` on a `chan` answers queued older searches on that channel as
  `superseded`, keeping one response per request.
- **`about`.** The daemon keeps its last 32 search responses, so `pick` and
  `report` can point at one instead of resending the query, reading and
  shown list.

---

## 4. Bindings

| Binding | Status | Notes |
|---|---|---|
| Rust crate | built | anyrun-style plugins or other Rust hosts link it directly |
| CLI + stdio protocol | built | usable from QML, GJS, Lua, shell, Go, Node and Python with no build step |
| Python (pyo3) | not built | would suit Albert, Ulauncher and Flow Launcher |
| WASM | not built | the core builds without `fs` and opens from bytes (`from_bytes`), which a WASM build needs; the `core` or `lite` set keeps the download small |
| C ABI | not built | protocol JSON in-process, for C/C++ hosts |

---

## 5. Integrations

| Front-end | Mechanism | Live per keystroke? | Status |
|---|---|---|---|
| **Quickshell** | `Process` + protocol v1 | yes | **built**: `integrations/quickshell/EmoticondService.qml`, a singleton (lifecycle, `set_defaults`, search, browse, picks, both report menus) |
| **fuzzel, rofi, wofi, tofi, bemenu, dmenu, walker** | dmenu mode via `emoticond menu` | no | **built** |
| Alfred (macOS) | Script Filter JSON via `--format alfred` | yes (re-runs the script) | output format built |
| anyrun | Rust `cdylib` plugin linking the crate | yes | not built |
| KRunner, GNOME Shell search | a D-Bus adapter | yes | not built |
| walker / elephant native provider | Go provider | yes | not built; `menu` covers walker today |
| Ulauncher, Albert, Flow Launcher | Python or JSON-RPC plugins | yes | not built |
| Raycast, Vicinae | TS extensions; CLI on Linux, WASM elsewhere | yes | not built |
| ags / astal (GJS) | `Gio.Subprocess` + protocol v1 | yes | no packaged example |
| fcitx5, ibus | input-method addons or static QuickPhrase files | addon: yes | not built |

**Japanese.** One index already parses Japanese keys (`嬉しい` reads as
happy). That makes input-method integrations (fcitx5, ibus) a good fit:
kaomoji are traditionally entered through the IME in Japan.

---

## 6. Static export

Not built. A static export (SQLite/JSON, rofimoji, fcitx5 QuickPhrase,
espanso files) would have to come from the library, not a separate
script, so it can't drift from the engine, and would ship the **engine's
own answers** for known terms (a top-N table per vocabulary term and
phrase), since a fuzzy filter over tags can't do what the engine does
(`idk` returns shrugs). Until then, `emoticond search --format jsonl` or
`--format dmenu` covers scripted use.

---

## 7. UI guidance for front-ends

Settings pages are in options.md §8. This section covers the search UI.

### 7.1 Result rendering

- **Kaomoji get full-width list rows.** No grid and no ellipsis: a cut-off face
  is a different face. For long faces (over about 24 columns), shrink the font
  one step first, then wrap. Use a font stack with broad coverage (Noto).
- Optionally show dimmed secondary text: the top 2 emotions
  (`fields:["emotions"]`) or the concepts the face is canonical for
  (`canonical_for`). Use the same text as the accessible label, because
  screen readers read kaomoji character by character.
- Keep the engine's order. Don't re-sort on the client. Use `id` as the row
  key.
- Keep the previous results on screen until the new response arrives, so the
  list never flashes empty. With queries this fast there is no need to
  debounce. Front-ends that spawn a process per query should debounce about
  50 ms.
- A face you mix in from elsewhere (emoji) goes in its own rows; the engine's
  scores don't compare with anything outside one response.

### 7.2 Reading line and correction

- One line under the search box: `Read as: very angry · pair`. Show it only
  when `reading.mode.kind` isn't `glyph` or `empty`. Update it together with
  the results, never ahead of them.
- On a correction: *Showing results for **angry**. Search instead for
  "angyr"*. The second part re-runs the search with `opts.correct=false`.
- A partial query (`mode: partial`) shows `completing: sad, salty, sassy`.
  Clicking a completion fills it in.

### 7.3 The query menu

The query menu is attached to the reading line.

```
How did this search go?
  ( ) Interpreted my search well
  ( ) Interpreted it incorrectly
  ( ) Didn't return what I wanted
  [ Add a note...                    ]
  ─────────────────────────────────────
  <disclaimer short text, from the ready line>
```

- The footer shows the **shipped** disclaimer text (`disclaimer.short`). It is
  never written by the front-end. When the shipped version is newer than
  `feedback.disclaimer_seen`, show the full text once before creating the
  report (options.md §5.4). `disclaimer.short` already matches the
  sending state.
- While the ready line says `"sending_off_by":"unasked"`, ask "send
  reports?" once in the menu (a yes and a no, neither preselected) and send
  the answer with `consent` (protocol.md).
- Choosing an item sends it at once. Choosing it again sends `clear` with
  `clears` set to that reason. The current choice stays visible while these
  results are shown. The note goes with the chosen reason, or as `note` if
  none is chosen. It is capped at `feedback.custom_max_chars`.

### 7.4 The face menu

Open it with right-click, the menu key or Shift+F10, or a `⋯` button shown
on hover or selection.

```
  Really good fit
  Fits, but not the word I used
  Doesn't fit
  Offensive / explicit (hides it)
  Add a note...
  ──────────────
  <disclaimer short text>
  Copy · Similar faces
```

- `Offensive` removes the row immediately and shows a toast, *Hidden. Undo*.
  Undo sends `clear` (or `unblock`), which unblocks the face. A marked row
  keeps a small badge, so a page of marks stays readable.
- `Similar faces` runs `similar`. *Show me others* re-runs the query with the
  shown ids in `opts.exclude`.

### 7.5 The empty query

Call `browse` (or `search` with an empty `q`): the canonical starter faces
first, then the user's history, each result tagged `from`. A first-time user
sees good faces straight away; a returning one sees their own.

### 7.6 Activation

Enter copies the face (and pastes or types it if the front-end can), then
sends `pick` with `about` and `rank`. Alt+Enter runs the other action
(copy or type). Front-ends never loosen safety on their own. They expose the
options.md settings instead.
