//! emoticond -- the command line (cli.rs, menu.rs) and the search daemon
//! (`emoticond serve`, proto.rs; docs/protocol.md). Config and state come
//! from emoticond-config and emoticond-state (app.rs). This file finds and
//! opens the data.
//!
//! The data is one file (format 3, docs/format.md): $EMOTICOND_DATA_FILE if
//! set; else, in a development checkout, <engine dir>/full.kmj, (re)compiled
//! at start (emoticond-compile) when it is missing or older than any of its
//! inputs: the export in the engine dir (from the data pipeline),
//! data/*.jsonl, data/grammar/, data/licence/ and data/situations.json; else
//! the config's data dirs and data set. The library itself only reads the file.
//!
//! The checkout is $EMOTICOND_REPO, else the one this binary was built in
//! (<repo>/engine/target/release/emoticond); the engine dir is
//! $EMOTICOND_ENGINE, else <repo>/work/engine.

mod app;
mod cli;
mod fetch;
mod menu;
mod out;
mod proto;

use emoticond::{Database, OpenOptions};
use emoticond_config::Resolved;
use std::time::Instant;

struct Cfg {
    repo: String,
    engine: String,
    /// $EMOTICOND_DATA_FILE: a .kmj to open as is (never compiled)
    data_file: Option<String>,
}

fn config() -> Cfg {
    let repo = std::env::var("EMOTICOND_REPO").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent()?.parent()?.parent()?.parent().map(|x| x.to_path_buf()))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into())
    });
    Cfg {
        engine: std::env::var("EMOTICOND_ENGINE").unwrap_or_else(|_| format!("{repo}/work/engine")),
        data_file: std::env::var("EMOTICOND_DATA_FILE").ok().filter(|s| !s.is_empty()),
        repo,
    }
}

/// What to open: $EMOTICOND_DATA_FILE as is; else, in a development
/// checkout (an export in <engine dir>), <engine dir>/full.kmj, compiled
/// from the export and data/ first when it is missing or older than any
/// input. It is written beside and then renamed, so a running engine keeps
/// its old mapping. Errors are reported and leave the old file alone. Outside a
/// checkout: the config's data search path (`EMOTICOND_DATA`, `data.dirs`,
/// then the platform dirs) and data set (`data.dataset`, core by default).
fn open_options(cfg: &Cfg, res: &Resolved) -> OpenOptions {
    if let Some(f) = &cfg.data_file {
        return OpenOptions::file(f);
    }
    let engine = std::path::Path::new(&cfg.engine);
    let path = engine.join("full.kmj");
    let inputs = emoticond_compile::legacy::LegacyPaths::for_repo(std::path::Path::new(&cfg.repo), engine);
    if !inputs.has_export() {
        if path.exists() {
            return OpenOptions::file(path);
        }
        let mut o = OpenOptions::default();
        o.data_dirs = res.data_dirs.clone();
        o.dataset = res.config.data.dataset;
        return o;
    }
    if inputs.is_stale(&path) {
        let res = inputs
            .read(emoticond_compile::Params::default())
            .and_then(|s| emoticond_compile::compile(&s))
            .and_then(|bytes| Ok(emoticond_compile::write_atomic(&path, &bytes)?));
        if let Err(e) = res {
            eprintln!("emoticond: could not build {}: {e}", path.display());
        }
    }
    OpenOptions::file(path)
}

/// The result of [`build`].
struct Built {
    db: Option<Database>,
    /// How long finding (and compiling) and opening took: the ready line's `ms`.
    ms: u128,
    /// A data file was there but could not be opened (the error is printed).
    failed: bool,
    /// Where it looked, when nothing was found.
    searched: Vec<std::path::PathBuf>,
}

/// Open the data, compiling it first if it is stale.
fn build(res: &Resolved) -> Built {
    let t0 = Instant::now();
    let (db, failed, searched) = match Database::open(open_options(&config(), res)) {
        Ok(d) => (Some(d), false, Vec::new()),
        Err(emoticond::OpenError::NotFound { searched }) => (None, false, searched),
        Err(e) => {
            eprintln!("emoticond: {e}");
            (None, true, Vec::new())
        }
    };
    Built { db, ms: t0.elapsed().as_millis(), failed, searched }
}

/// Overlay `extra` onto `v`, recursing into objects (so `{"styles":{"lenny":"hide"}}`
/// keeps the other styles).
fn merge(v: &mut serde_json::Value, extra: &serde_json::Map<String, serde_json::Value>) {
    let serde_json::Value::Object(m) = v else { return };
    for (k, x) in extra {
        match (m.get_mut(k), x) {
            (Some(cur @ serde_json::Value::Object(_)), serde_json::Value::Object(sub)) => merge(cur, sub),
            _ => {
                m.insert(k.clone(), x.clone());
            }
        }
    }
}

/// Exits the process after `idle_exit` seconds with no request. A
/// request in progress holds the lock, so the exit never cuts one short.
struct Idle {
    last: std::sync::Arc<std::sync::Mutex<Instant>>,
}

impl Idle {
    /// Held while a request is handled; marks the time when dropped.
    fn busy(&self) -> IdleGuard<'_> {
        IdleGuard(self.last.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

struct IdleGuard<'a>(std::sync::MutexGuard<'a, Instant>);

impl Drop for IdleGuard<'_> {
    fn drop(&mut self) {
        *self.0 = Instant::now();
    }
}

fn idle_exit(secs: u64) -> Option<Idle> {
    if secs == 0 {
        return None;
    }
    let limit = std::time::Duration::from_secs(secs);
    let last = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
    let watch = last.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(t) = watch.try_lock() {
            if t.elapsed() >= limit {
                std::process::exit(0);
            }
        }
    });
    Some(Idle { last })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(cli::main(&args))
}
