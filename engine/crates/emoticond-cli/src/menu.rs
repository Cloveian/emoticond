//! `emoticond menu`: the whole pick flow for dmenu-style launchers
//! (docs/api-frontends.md §2.3).
//!
//! 1. **Ask.** The launcher opens with the browse list (canonical starter
//!    faces, then your history). Enter on a row picks that face; text that
//!    matches no row is taken as a query.
//! 2. **Search** with the user's config (`[profile.menu]`) and show the
//!    results in the same launcher.
//! 3. **Act**: copy (`wl-copy`, or `xclip`/`xsel` on X11), type (`wtype`,
//!    `xdotool`, `ydotool`) or print, then record the pick.
//!
//! The launcher is `--launcher`, else `$EMOTICOND_LAUNCHER`, else the first of
//! fuzzel, rofi, walker, wofi, tofi, bemenu, dmenu that is installed. A name
//! not in the table is run as a command line (its words as given). Esc in
//! the launcher exits 130.

use super::app::App;
use super::cli::{record_pick, Args, CANCELLED, IO, NO_RESULTS, OK, USAGE};
use super::out::HitOut;
use emoticond::{Database, Hit};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Each launcher's flags: (binary, ask step, pick step). `{p}` is the prompt.
const LAUNCHERS: &[(&str, &[&str], &[&str])] = &[
    ("fuzzel", &["--dmenu", "--prompt", "{p} "], &["--dmenu", "--prompt", "{p} "]),
    ("rofi", &["-dmenu", "-i", "-p", "{p}"], &["-dmenu", "-i", "-no-custom", "-p", "{p}"]),
    ("walker", &["--dmenu"], &["--dmenu"]),
    ("wofi", &["--dmenu", "--prompt", "{p}"], &["--dmenu", "--prompt", "{p}"]),
    ("tofi", &["--prompt-text", "{p} ", "--require-match=false"], &["--prompt-text", "{p} "]),
    ("bemenu", &["-i", "-p", "{p}"], &["-i", "-p", "{p}"]),
    ("dmenu", &["-i", "-p", "{p}"], &["-i", "-p", "{p}"]),
];

fn on_path(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        p.is_file()
    })
}

/// The launcher to use: (program, ask args, pick args).
fn launcher(choice: Option<&str>) -> Result<(PathBuf, Vec<String>, Vec<String>), String> {
    let env = std::env::var("EMOTICOND_LAUNCHER").ok().filter(|s| !s.trim().is_empty());
    let choice = choice.map(String::from).or(env).filter(|c| c != "auto");
    let own = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match choice {
        None => LAUNCHERS
            .iter()
            .find_map(|(b, ask, pick)| on_path(b).map(|p| (p, own(ask), own(pick))))
            .ok_or_else(|| "no launcher found (tried fuzzel, rofi, walker, wofi, tofi, bemenu, dmenu); use --launcher".into()),
        Some(c) => {
            if let Some((b, ask, pick)) = LAUNCHERS.iter().find(|(b, ..)| *b == c) {
                return on_path(b).map(|p| (p, own(ask), own(pick))).ok_or_else(|| format!("launcher {b:?} is not installed"));
            }
            let mut words = c.split_whitespace();
            let bin = words.next().ok_or("empty --launcher")?;
            let args: Vec<String> = words.map(String::from).collect();
            let p = on_path(bin).ok_or_else(|| format!("launcher {bin:?} not found"))?;
            Ok((p, args.clone(), args))
        }
    }
}

/// Run the launcher over `lines`; `None` when it was cancelled.
fn choose(bin: &PathBuf, args: &[String], prompt: &str, lines: &[String]) -> Result<Option<String>, String> {
    let args: Vec<String> = args.iter().map(|a| a.replace("{p}", prompt)).collect();
    let mut child = Command::new(bin)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let mut s = lines.join("\n");
        s.push('\n');
        // a launcher may exit before reading everything; that is fine
        let _ = stdin.write_all(s.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("{}: {e}", bin.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string();
    if !out.status.success() || text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(text))
}

fn row(db: &Database, h: &Hit) -> String {
    let hint = HitOut::hint(db, h.id);
    if hint.is_empty() {
        h.text.clone()
    } else {
        format!("{}  ·  {hint}", h.text)
    }
}

/// Which row the launcher returned, if any (launchers may trim).
fn which(rows: &[String], got: &str) -> Option<usize> {
    rows.iter().position(|r| r == got).or_else(|| rows.iter().position(|r| r.trim() == got.trim()))
}

fn act(action: &str, text: &str) -> Result<(), String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let tools: &[(&str, &[&str], bool)] = match action {
        // (program, args, text on stdin rather than as the last argument)
        "copy" if wayland => &[("wl-copy", &[], true), ("xclip", &["-selection", "clipboard"], true), ("xsel", &["--clipboard", "--input"], true)],
        "copy" => &[("xclip", &["-selection", "clipboard"], true), ("xsel", &["--clipboard", "--input"], true), ("wl-copy", &[], true)],
        "type" if wayland => &[("wtype", &["--"], false), ("ydotool", &["type", "--"], false), ("xdotool", &["type", "--clearmodifiers", "--"], false)],
        "type" => &[("xdotool", &["type", "--clearmodifiers", "--"], false), ("ydotool", &["type", "--"], false), ("wtype", &["--"], false)],
        _ => {
            println!("{text}");
            return Ok(());
        }
    };
    let Some((bin, args, stdin)) = tools.iter().find_map(|(b, a, s)| on_path(b).map(|p| (p, a, s))) else {
        let names: Vec<&str> = tools.iter().map(|t| t.0).collect();
        return Err(format!("nothing to {action} with: install one of {}", names.join(", ")));
    };
    let mut cmd = Command::new(&bin);
    cmd.args(*args);
    if *stdin {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.arg(text);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", bin.display()))?;
    if *stdin {
        if let Some(mut s) = child.stdin.take() {
            s.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {st}", bin.display()))
    }
}

pub fn run(db: &Database, app: &mut App, a: &Args) -> Result<i32, i32> {
    let action = a.value("--action").unwrap_or("copy");
    if !matches!(action, "copy" | "type" | "print") {
        eprintln!("emoticond menu: --action is copy, type or print");
        return Err(USAGE);
    }
    let prompt = a.value("--prompt").unwrap_or("kaomoji");
    let (bin, ask, pick) = launcher(a.value("--launcher")).map_err(|e| {
        eprintln!("emoticond menu: {e}");
        IO
    })?;
    let opts = {
        let keep = a.opts.contains_key("usage");
        let (mut o, w) = app.options_with(&a.opts).map_err(|e| {
            eprintln!("emoticond: {e}");
            USAGE
        })?;
        for x in w {
            eprintln!("emoticond: warning: {}", x.message);
        }
        app.finish(&mut o, keep, None);
        o
    };
    let fail = |e: String| {
        eprintln!("emoticond menu: {e}");
        IO
    };

    // 1. ask, over the browse list
    let start = db.browse(&opts);
    let rows: Vec<String> = start.hits.iter().map(|h| row(db, h)).collect();
    let Some(got) = choose(&bin, &ask, prompt, &rows).map_err(fail)? else { return Ok(CANCELLED) };
    let (query, hit) = match which(&rows, &got) {
        Some(i) => (String::new(), start.hits[i].clone()),
        None => {
            // 2. search, and show the results
            let q = got.trim().to_string();
            let r = db.search(&q, &opts);
            if r.hits.is_empty() {
                eprintln!("emoticond menu: nothing found for {q:?}");
                return Ok(NO_RESULTS);
            }
            let rows: Vec<String> = r.hits.iter().map(|h| row(db, h)).collect();
            let Some(got) = choose(&bin, &pick, &q, &rows).map_err(fail)? else { return Ok(CANCELLED) };
            match which(&rows, &got) {
                Some(i) => (q, r.hits[i].clone()),
                None => return Ok(CANCELLED),
            }
        }
    };

    // 3. act, then record
    act(action, &hit.text).map_err(fail)?;
    if let Some(e) = db.get(hit.id) {
        record_pick(db, app, &e, &query, &opts, Some(hit.rank))?;
    }
    Ok(OK)
}
