//! Fold a dev pick log (`dev.pick_log`) into a `usage.json`.
//!
//! ```text
//! cargo run -p emoticond-state --example import_picks -- work/eval/picks.jsonl [usage.json]
//! ```
//!
//! Without the second argument it only reports what it would import. The
//! concept for each pick is the normalised query (the engine's `term_key`
//! would be better; a front-end with a `Database` at hand can pass that
//! instead).

use emoticond_state::{import_picks, normalize_query, now_ms, UsageSettings, UsageStore};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(src) = args.next().map(PathBuf::from) else {
        eprintln!("usage: import_picks <picks.jsonl> [usage.json]");
        std::process::exit(2);
    };
    let dest = args.next().map(PathBuf::from);
    let run = |st: &mut emoticond::UsageState| match import_picks(&src, st, normalize_query) {
        Ok(s) => {
            for w in &s.warnings {
                eprintln!("warning: {}", w.message);
            }
            println!("imported {} picks, skipped {}", s.imported, s.skipped);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    match dest {
        None => {
            let mut st = emoticond::UsageState::default();
            run(&mut st);
            println!("{} faces, {} concepts (dry run; pass a usage.json to write)", st.global.len(), st.by_term.len());
        }
        Some(dest) => {
            let outbox = dest.parent().map(|d| d.join("outbox")).unwrap_or_else(|| PathBuf::from("outbox"));
            let mut store = UsageStore::open(&dest, outbox, UsageSettings::default());
            if let Err(e) = store.update(now_ms(), run) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!("wrote {}", dest.display());
        }
    }
}
