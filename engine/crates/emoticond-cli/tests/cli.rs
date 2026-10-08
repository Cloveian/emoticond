//! The `emoticond` command line: arguments, output formats, exit codes
//! (docs/api-frontends.md §2), and the state it writes.

mod common;
use common::*;

#[test]
fn usage_errors_exit_2() {
    let s = Sandbox::new(None);
    assert_eq!(code(&s.run(&[])), 2);
    assert_eq!(code(&s.run(&["frobnicate"])), 2);
    assert_eq!(code(&s.run(&["search", "--bogus", "x"])), 2);
    assert_eq!(code(&s.run(&["search", "--opts", "[1]", "x"])), 2);
    assert_eq!(code(&s.run(&["search", "--safety", "spicy", "x"])), 2, "a bad value for a real option");
    assert_eq!(code(&s.run(&["search", "--limit"])), 2, "a flag missing its value");
    assert_eq!(code(&s.run(&["serve", "--socket", "/tmp/x"])), 2);
    assert_eq!(code(&s.run(&["config", "frob"])), 2);
}

#[test]
fn help_version_and_config_need_no_data() {
    let s = Sandbox::new(None);
    let h = s.run(&["--help"]);
    assert_eq!(code(&h), 0);
    assert!(stdout(&h).contains("emoticond menu"));
    let v = s.run(&["--version"]);
    assert_eq!(code(&v), 0);
    assert!(stdout(&v).starts_with("emoticond "));
    s.config("[search]\nsafety = \"moderate\"\n[profile.cli.search]\nlimit = 7\n");
    let c = s.run(&["config", "show"]);
    assert_eq!(code(&c), 0, "{}", stderr(&c));
    let out = stdout(&c);
    assert!(out.contains("# profile: cli"), "{out}");
    assert!(out.lines().any(|l| l.starts_with("search.safety = \"moderate\"")), "{out}");
    assert!(out.lines().any(|l| l.starts_with("search.limit = 7") && l.contains("[profile.cli]")), "{out}");
    // a command-line flag beats the file
    let c = s.run(&["config", "--safety", "off"]);
    assert!(stdout(&c).lines().any(|l| l.starts_with("search.safety = \"off\"") && l.contains("command line")));
}

#[test]
fn missing_data_exits_3() {
    let s = Sandbox::new(None);
    let o = s.run(&["search", "idk"]);
    assert_eq!(code(&o), 3, "{}", stderr(&o));
    assert!(stderr(&o).contains("no kaomoji data"));
}

#[test]
fn search_formats_and_exit_codes() {
    let Some(repo) = data("search_formats_and_exit_codes") else { return };
    let s = Sandbox::new(Some(&repo));
    let o = s.run(&["search", "idk", "-n", "3"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let lines: Vec<String> = stdout(&o).lines().map(String::from).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "¯\\_(ツ)_/¯");

    assert_eq!(code(&s.run(&["search", "zqxjv", "wqpz"])), 1, "no results is exit 1");

    let o = s.run(&["search", "--dmenu", "-n", "2", "shrug"]);
    for l in stdout(&o).lines() {
        let f: Vec<&str> = l.split('\t').collect();
        assert_eq!(f.len(), 3, "{l}");
        assert!(f[2].starts_with('k') && f[2].len() == 13, "{l}");
    }

    let o = s.run(&["search", "--format", "tsv", "-n", "2", "sad"]);
    assert!(stdout(&o).lines().all(|l| l.split('\t').nth(1) == Some("emoticon")));

    let o = s.run(&["search", "--json", "--fields", "flags,emotions", "-n", "4", "--explain", "reading", "sad"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["n"], 4);
    assert_eq!(v["term_key"], "sad");
    assert!(v["reading"]["line"].as_str().unwrap().contains("sad"));
    assert!(v["results"][0]["emotions"]["sad"].as_f64().unwrap() > 0.5);
    assert!(v["results"][0]["flags"].as_array().unwrap().iter().any(|f| f == "pinned"));

    let o = s.run(&["search", "--format", "jsonl", "-n", "2", "happy"]);
    assert_eq!(stdout(&o).lines().filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()).count(), 2);

    // per-option flags reach the query
    let lewd = s.run(&["search", "--json", "-n", "50", "--safety", "strict", "lewd"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&lewd).trim()).unwrap();
    assert!(v["results"].as_array().unwrap().len() <= 50);

    // an empty query browses: canonical starter faces first
    let o = s.run(&["search", "--json", "-n", "3", ""]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v["mode"], "browse");
    assert_eq!(v["results"][0]["from"], "canonical");

    // the query from stdin
    let mut c = s.cmd(EMOTICOND);
    c.args(["search", "-n", "1", "-"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b"idk\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(stdout(&o).trim(), "¯\\_(ツ)_/¯");
}

#[test]
fn strict_options_refuse_unknown_keys() {
    let Some(repo) = data("strict_options_refuse_unknown_keys") else { return };
    let s = Sandbox::new(Some(&repo));
    let o = s.run(&["search", "--opts", r#"{"bogus":1}"#, "idk"]);
    assert_eq!(code(&o), 0);
    assert!(stderr(&o).contains("bogus"));
    assert_eq!(code(&s.run(&["search", "--strict-options", "--opts", r#"{"bogus":1}"#, "idk"])), 2);
}

#[test]
fn get_explain_similar_complete() {
    let Some(repo) = data("get_explain_similar_complete") else { return };
    let s = Sandbox::new(Some(&repo));
    let o = s.run(&["get", "k2026da3e4989", "¯\\_(ツ)_/¯"]);
    assert_eq!(code(&o), 0);
    assert_eq!(stdout(&o).lines().count(), 2);
    assert!(stdout(&o).contains("canonical: "));
    let o = s.run(&["get", "--json", "k2026da3e4989", "no such face"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v[0]["text"], "¯\\_(ツ)_/¯");
    assert!(v[1].is_null());
    assert_eq!(code(&s.run(&["get", "not a face at all"])), 1);

    let o = s.run(&["explain", "shy", "proud"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("shy") && stdout(&o).contains("proud"));
    assert_eq!(code(&s.run(&["explain"])), 2);

    let o = s.run(&["similar", "-n", "5", "k2026da3e4989"]);
    assert_eq!(code(&o), 0);
    assert_eq!(stdout(&o).lines().count(), 5);

    let o = s.run(&["complete", "-n", "3", "emba"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).lines().all(|l| l.starts_with("emba")), "{}", stdout(&o));
}

#[test]
fn pick_records_usage_and_the_dev_log() {
    let Some(repo) = data("pick_records_usage_and_the_dev_log") else { return };
    let s = Sandbox::new(Some(&repo));
    let o = s.run(&["pick", "╮(╯_╰)╭", "--query", "idk"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let usage: serde_json::Value = serde_json::from_str(&s.read(s.state().join("usage.json"))).unwrap();
    assert!(usage["by_term"]["idk"]["k7fff199b7a51"].as_f64().unwrap() > 0.0, "{usage}");
    let log = s.read(s.picks());
    let rec: serde_json::Value = serde_json::from_str(log.trim()).unwrap();
    assert_eq!(rec["q"], "idk");
    assert_eq!(rec["text"], "╮(╯_╰)╭");
    assert_eq!(rec["kind"], "emoticon");
    assert!(rec["ts"].as_f64().unwrap() > 1e9);
    assert!(rec["shown"].as_array().unwrap().len() > 1);

    // history shows up after the canonical faces on an empty query
    let o = s.run(&["browse", "--recent", "--json"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v["results"][0]["text"], "╮(╯_╰)╭");
    assert_eq!(v["results"][0]["from"], "history");

    // popularity off: nothing recorded, still exit 0
    let s2 = Sandbox::new(Some(&repo));
    let o = s2.run(&["pick", "--popularity-mode", "off", "╮(╯_╰)╭"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(!s2.state().join("usage.json").exists());
    assert_eq!(code(&s.run(&["pick", "not a face"])), 1);
}

#[test]
fn reports_queue_and_apply_local_effects() {
    let Some(repo) = data("reports_queue_and_apply_local_effects") else { return };
    let s = Sandbox::new(Some(&repo));
    let top = |s: &Sandbox| -> Vec<String> {
        let o = s.run(&["search", "--json", "-n", "5", "sad"]);
        ids(&serde_json::from_str(stdout(&o).trim()).unwrap())
    };
    let before = top(&s);
    let victim = before[0].clone();

    // bad reports
    assert_eq!(code(&s.run(&["report"])), 2);
    assert_eq!(code(&s.run(&["report", "--reason", "nope", "--query", "sad"])), 2);
    assert_eq!(code(&s.run(&["report", "--reason", "missing"])), 2, "a query reason needs --query");
    assert_eq!(code(&s.run(&["report", "--reason", "offensive", "--query", "sad"])), 2, "a face reason needs --face");
    assert_eq!(code(&s.run(&["report", "--reason", "note", "--query", "sad"])), 2, "a note needs text");

    // offensive: blocked, hidden from the next search, queued with context
    let o = s.run(&["report", "--json", "--reason", "offensive", "--face", &victim, "--query", "sad"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let ack: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(ack["queued"], true);
    assert_eq!(ack["sent"], false);
    assert_eq!(ack["effects"], serde_json::json!(["blocked"]));
    assert!(s.read(s.state().join("blocklist.txt")).contains(&victim));
    assert!(!top(&s).contains(&victim));
    let q = s.read(s.state().join("reports/queue.jsonl"));
    let rep: serde_json::Value = serde_json::from_str(q.lines().last().unwrap()).unwrap();
    assert_eq!(rep["reason"], "offensive");
    assert_eq!(rep["query"], "sad");
    assert_eq!(rep["term_key"], "sad");
    assert!(rep["reading"].as_str().unwrap().contains("sad"));
    assert_eq!(rep["shown"].as_array().unwrap().len(), 20, "the top 20 shown");
    assert_eq!(rep["target"]["id"], victim.as_str());

    // clear undoes it
    let o = s.run(&["report", "--reason", "clear", "--face", &victim, "--query", "sad"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("unblocked"));
    assert_eq!(top(&s), before);

    // no fit: a demote row for the concept; great fit: a pick
    let o = s.run(&["report", "--reason", "no_fit", "--face", &before[1], "--query", "sad"]);
    assert!(stdout(&o).contains("demoted"), "{}", stdout(&o));
    assert!(s.read(s.state().join("overlays/boosts.jsonl")).contains("\"boost\":-3"));
    let o = s.run(&["report", "--reason", "great_fit", "--face", &before[2], "--query", "sad"]);
    assert!(stdout(&o).contains("picked"));
    assert!(s.read(s.state().join("usage.json")).contains(&before[2]));

    // sending off: still queued, the footer says so
    let o = s.run(&["report", "--feedback-send", "false", "--reason", "missing", "--query", "sad", "--note", "more tears"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stdout(&o).contains("saved on this computer only"), "{}", stdout(&o));
    let q = s.read(s.state().join("reports/queue.jsonl"));
    assert!(q.lines().last().unwrap().contains("more tears"));

    // block / unblock directly
    assert_eq!(code(&s.run(&["block", &before[3]])), 0);
    assert!(!top(&s).contains(&before[3]));
    assert_eq!(code(&s.run(&["unblock", &before[3]])), 0);
    assert!(top(&s).contains(&before[3]));
}

#[test]
fn menu_runs_the_two_step_flow() {
    let Some(repo) = data("menu_runs_the_two_step_flow") else { return };
    let s = Sandbox::new(Some(&repo));
    // a fake launcher: the first time it "types" a query, then picks the
    // first row it is shown
    let bin = s.root.join("fake-launcher");
    let flag = s.root.join("asked");
    std::fs::write(
        &bin,
        format!("#!/bin/sh\nif [ -e {f} ]; then head -n 1; else cat >/dev/null; touch {f}; echo idk; fi\n", f = flag.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = s.run(&["menu", "--launcher", bin.to_str().unwrap(), "--action", "print"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "¯\\_(ツ)_/¯");
    assert!(s.read(s.state().join("usage.json")).contains("k2026da3e4989"));

    // Esc: the launcher exits 1 with nothing
    let esc = s.root.join("esc");
    std::fs::write(&esc, "#!/bin/sh\ncat >/dev/null\nexit 1\n").unwrap();
    std::fs::set_permissions(&esc, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&s.run(&["menu", "--launcher", esc.to_str().unwrap(), "--action", "print"])), 130);
    // a launcher that isn't there
    assert_eq!(code(&s.run(&["menu", "--launcher", "/nonexistent/launcher"])), 4);
    assert_eq!(code(&s.run(&["menu", "--action", "paste"])), 2);
}

#[test]
fn info_lists_paths() {
    let Some(repo) = data("info_lists_paths") else { return };
    let s = Sandbox::new(Some(&repo));
    let o = s.run(&["info", "--json"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v["protocol"], 1);
    assert_eq!(v["popularity"], "local");
    assert_eq!(v["state"]["usage"], s.state().join("usage.json").to_str().unwrap());
    assert_eq!(v["dev"]["pick_log"], s.picks().to_str().unwrap());
}
