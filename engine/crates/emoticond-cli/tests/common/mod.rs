//! Running the binaries in a sandbox: a temp HOME with its own XDG dirs
//! (state, config), dev logs in the sandbox, and the real data when present
//! (`$EMOTICOND_TEST_REPO` or `$EMOTICOND_REPO`, else the checkout this crate
//! lives in). Without data, the tests that need it pass without checking
//! anything (and say so).
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub const EMOTICOND: &str = env!("CARGO_BIN_EXE_emoticond");

pub fn repo() -> Option<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let env = ["EMOTICOND_TEST_REPO", "EMOTICOND_REPO"].into_iter().filter_map(std::env::var_os).map(PathBuf::from);
    env.chain([here]).find(|r| r.join("work/engine/faces.jsonl").exists() && r.join("data/canonical.jsonl").exists())
}

/// The data checkout, or `None` after saying the test is skipped.
pub fn data(test: &str) -> Option<PathBuf> {
    let r = repo();
    if r.is_none() {
        eprintln!("{test}: no data checkout (set EMOTICOND_TEST_REPO); skipped");
    }
    r
}

/// A sandbox: its own HOME, XDG dirs and dev logs.
pub struct Sandbox {
    pub root: PathBuf,
    pub repo: PathBuf,
}

impl Sandbox {
    pub fn new(repo: Option<&Path>) -> Sandbox {
        static N: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "emoticond-cli-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        Sandbox { repo: repo.map(Path::to_path_buf).unwrap_or_else(|| root.join("no-data")), root }
    }

    pub fn state(&self) -> PathBuf {
        self.root.join("state/emoticond")
    }
    pub fn picks(&self) -> PathBuf {
        self.root.join("picks.jsonl")
    }
    pub fn read(&self, p: impl AsRef<Path>) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    pub fn cmd(&self, bin: &str) -> Command {
        let mut c = Command::new(bin);
        for (k, _) in std::env::vars_os() {
            if let Some(k) = k.to_str() {
                if k.starts_with("EMOTICOND") || k.starts_with("XDG_") {
                    c.env_remove(k);
                }
            }
        }
        c.env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CONFIG_DIRS", self.root.join("etc/xdg"))
            .env("XDG_DATA_DIRS", self.root.join("usr/share"))
            .env("EMOTICOND_REPO", &self.repo)
            .env("EMOTICOND_PICK_LOG", self.picks())
            // never send the tests' reports anywhere
            .env("EMOTICOND_REPORT_ENDPOINT", "none")
            .env("EMOTICOND_STATS_ENDPOINT", "none");
        c
    }

    /// Run `emoticond ARGS`.
    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(EMOTICOND).args(args).stdin(Stdio::null()).output().unwrap()
    }

    pub fn config(&self, toml: &str) {
        let d = self.root.join("config/emoticond");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.toml"), toml).unwrap();
    }

    /// Start `bin` with `args` speaking the protocol.
    pub fn serve(&self, bin: &str, args: &[&str]) -> Daemon {
        let mut child = self.cmd(bin).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut d = Daemon { child, stdin: Some(stdin), stdout, ready: serde_json::Value::Null, ready_line: String::new() };
        d.ready_line = d.line();
        d.ready = serde_json::from_str(&d.ready_line).unwrap();
        d
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}
pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub struct Daemon {
    pub child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    pub ready: serde_json::Value,
    pub ready_line: String,
}

impl Daemon {
    pub fn send(&mut self, line: &str) {
        let s = self.stdin.as_mut().unwrap();
        writeln!(s, "{line}").unwrap();
        s.flush().unwrap();
    }

    pub fn line(&mut self) -> String {
        let mut l = String::new();
        self.stdout.read_line(&mut l).unwrap();
        assert!(!l.is_empty(), "daemon closed its output");
        l.trim_end_matches('\n').to_string()
    }

    /// Send a request and parse the one response line.
    pub fn ask(&mut self, line: &str) -> serde_json::Value {
        self.send(line);
        let l = self.line();
        serde_json::from_str(&l).unwrap_or_else(|e| panic!("{e}: {l}"))
    }

    /// Close stdin and wait for the exit status.
    pub fn finish(mut self) -> i32 {
        drop(self.stdin.take());
        self.child.wait().unwrap().code().unwrap_or(-1)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn texts(v: &serde_json::Value) -> Vec<String> {
    v["results"].as_array().map(|a| a.iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}
pub fn ids(v: &serde_json::Value) -> Vec<String> {
    v["results"].as_array().map(|a| a.iter().filter_map(|r| r["id"].as_str().map(String::from)).collect()).unwrap_or_default()
}
