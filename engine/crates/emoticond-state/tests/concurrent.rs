//! Several writers on the same files: threads with their own handles, and
//! separate processes (this test binary re-run as a child).

mod common;

use common::{report, TempDir};
use emoticond::{FaceId, Pick, Reason, Target};
use emoticond_state::{
    add_to_blocklist, load_blocklists, DevLog, FeedbackQueue, QueueLimits, StatePaths, UsageSettings, UsageStore,
};
use std::path::Path;

const N: usize = 40;

fn settings() -> UsageSettings {
    let mut s = UsageSettings::default();
    s.max_entries = 100_000;
    s
}

/// One writer's work: N reports, N picks of distinct faces, N blocklist
/// entries, N dev-log lines. All at the same `now`, so nothing decays.
fn writer(root: &Path, who: &str) {
    let p = StatePaths::at(root);
    let mut lim = QueueLimits::default();
    lim.queue_max = 100_000;
    let q = FeedbackQueue::new(&p, lim);
    let mut u = UsageStore::open_in(&p, settings());
    let log = DevLog::at(root.join("devlog.jsonl"));
    for i in 0..N {
        let text = format!("{who}-{i}");
        q.append(&report(&text, 1, &text, Target::Query, Reason::Missing), 1).unwrap();
        u.record(&Pick::new(FaceId::of_text(&text), Some(who.to_string())), 1).unwrap();
        add_to_blocklist(&p.blocklist, FaceId::of_text(&text)).unwrap();
        log.append(&serde_json::json!({ "q": text, "text": "x" }), 1).unwrap();
    }
}

fn check(root: &Path, writers: usize) {
    let p = StatePaths::at(root);
    let total = writers * N;
    let mut lim = QueueLimits::default();
    lim.queue_max = 100_000;
    let q = FeedbackQueue::new(&p, lim);
    let pending = q.pending(1).unwrap();
    assert!(pending.warnings.is_empty(), "{:?}", pending.warnings);
    assert_eq!(pending.len(), total);
    // no lost updates in the read-modify-write of usage.json
    let u = UsageStore::open_in(&p, settings());
    assert_eq!(u.state().global.len(), total);
    assert_eq!(u.state().by_term.len(), writers);
    assert!(u.state().global.values().all(|w| *w == 1.0));
    let bl = load_blocklists(&[&p.blocklist]);
    assert!(bl.warnings.is_empty());
    assert_eq!(bl.ids.len(), total);
    let log = std::fs::read_to_string(root.join("devlog.jsonl")).unwrap();
    assert_eq!(log.lines().count(), total);
    assert!(log.lines().all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()));
    // no temp files left behind
    let stray: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(stray.is_empty());
}

#[test]
fn threads_share_the_files() {
    let d = TempDir::new();
    let writers = 4;
    std::thread::scope(|s| {
        for w in 0..writers {
            let root = d.path();
            s.spawn(move || writer(root, &format!("t{w}")));
        }
    });
    check(d.path(), writers);
}

/// Only does anything when run as a child by `processes_share_the_files`.
#[test]
fn child_writer() {
    if let (Ok(root), Ok(who)) = (std::env::var("EMOTICOND_STATE_CHILD_ROOT"), std::env::var("EMOTICOND_STATE_CHILD_WHO")) {
        writer(Path::new(&root), &who);
    }
}

#[test]
fn processes_share_the_files() {
    let d = TempDir::new();
    let exe = std::env::current_exe().unwrap();
    let writers = 3;
    let children: Vec<_> = (0..writers)
        .map(|w| {
            std::process::Command::new(&exe)
                .args(["--exact", "child_writer", "--test-threads=1", "--quiet"])
                .env("EMOTICOND_STATE_CHILD_ROOT", d.path())
                .env("EMOTICOND_STATE_CHILD_WHO", format!("p{w}"))
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    check(d.path(), writers);
}
