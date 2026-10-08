# Development

building, testing, and where the data comes from

## build and test

```sh
cd engine
cargo build --release          # target/release/emoticond, emoticond-compile
cargo test
cargo clippy --all-targets
```

- needs rust 1.89+
- the core crate's tests run against a small fixture
  (`crates/emoticond/tests/fixtures/tiny.kmj`). if you change the format or
  the scoring, rebuild it with `EMOTICOND_BLESS_FIXTURE=1 cargo test -p
  emoticond-compile`
- tests that need the full data export say "skipped" and pass

to try a build against real data, fetch a file and point at it:

```sh
emoticond data fetch --dir /tmp/emo
EMOTICOND_DATA_FILE=/tmp/emo/core.kmj target/release/emoticond search eep
```

## crates

| crate | |
|---|---|
| `emoticond` | the library: open a `.kmj`, search, browse, similar. no I/O beyond reading the file |
| `emoticond-config` | config files, profiles, `/etc` policy, paths |
| `emoticond-state` | popularity, reports, blocklists, overlays |
| `emoticond-compile` | builds `.kmj` files from a data export and `data/` |
| `emoticond-cli` | the `emoticond` binary: cli, `serve`, `menu`, `data fetch` |
| `emoticond-collector` | the report server (docs/collector.md) |

## where the data comes from

the data files are built by a separate, private pipeline: scraping, the
training data, the models and the rating/judging rounds live there, and
only the results are published (as `.kmj` files in
[emoticond-data](https://github.com/Cloveian/emoticond-data)). docs/public-data.md
says what a public build may contain.

this repo has the hand-curated inputs the compiler reads:

| file | |
|---|---|
| `data/canonical.jsonl`, `data/canonical_hand.jsonl` | the faces that go first for plain concepts (`idk`, `shrug`) |
| `data/lexicon_phrases.jsonl` | search phrases and their emotion readings |
| `data/boosts.jsonl` | per-term boosts |
| `data/situations.json` | situations (`running late`) |
| `data/grammar/en.json` | intensity, negation, filler words |
| `data/licence/` | the licence summary and attribution baked into every file |

fixes to these are welcome as pull requests; they go into the next data
release.

## versions

data is `X.Y`, emoticond is `X.Y.Z`:

- **X:** compatibility. emoticond `X.*.*` only opens data `X.*`
- **Y:** the data release this emoticond was made for
- **Z:** changes that don't touch the data

a `.kmj` built locally is version `dev` and always opens.

## releasing

1. set the version in `engine/Cargo.toml` (`X.Y.Z`), then rebuild the test
   fixture, which records it: `EMOTICOND_BLESS_FIXTURE=1 cargo test -p
   emoticond-compile --test tiny`
2. a new Y: publish data `X.Y` to emoticond-data (the publish step checks
   the code is already at `X.Y.*`)
3. commit, then `git tag vX.Y.Z && git push --tags`
4. the release workflow builds the binaries and attaches them, with
   `SHA256SUMS`, to the GitHub release
