//! `emoticond serve` over stdin/stdout (docs/protocol.md),
//! with the real data when present.

mod common;
use common::*;
use serde_json::{json, Value};

fn serve(s: &Sandbox) -> Daemon {
    s.serve(EMOTICOND, &["serve", "--idle", "0"])
}

#[test]
fn ready_line_announces_v1() {
    let Some(repo) = data("ready_line_announces_v1") else { return };
    let s = Sandbox::new(Some(&repo));
    let d = serve(&s);
    let r = &d.ready;
    assert_eq!(r["ready"], true);
    assert!(d.ready_line.starts_with("{\"ready\":true,"), "{}", d.ready_line);
    assert!(r["ms"].is_u64());
    assert!(r["counts"]["emoticon"].as_u64().unwrap() > 1000);
    assert_eq!(r["counts"]["emoticon"], r["data"]["faces"]);
    for gone in ["engine", "emoticon", "protocols", "emoji", "symbol"] {
        assert!(r.get(gone).is_none(), "{gone} is gone");
    }
    assert_eq!(r["protocol"], 1);
    assert!(r["features"].as_array().unwrap().iter().any(|f| f == "report"));
    assert_eq!(r["disclaimer"]["version"], 1);
    assert!(r["disclaimer"]["text"].as_str().unwrap().contains("top 20"));
    assert_eq!(r["sending"], true);
    assert_eq!(r["popularity"], "local");
    assert_eq!(d.finish(), 0);
}

#[test]
fn search_with_ids_opts_fields_and_errors() {
    let Some(repo) = data("search_with_ids_opts_fields_and_errors") else { return };
    let s = Sandbox::new(Some(&repo));
    let mut d = serve(&s);
    let r = d.ask(r#"{"op":"search","id":1,"q":"idk","opts":{"limit":3,"explain":"reading"},"fields":["flags","emotions","quality"]}"#);
    assert_eq!(r["id"], 1);
    assert_eq!(r["ok"], true);
    assert_eq!(r["n"], 3);
    assert_eq!(r["term_key"], "idk");
    assert_eq!(r["results"][0]["id"], "k2026da3e4989");
    assert_eq!(r["results"][0]["text"], "¯\\_(ツ)_/¯");
    assert!(r["results"][0]["flags"].as_array().unwrap().contains(&json!("pinned")));
    assert!(r["results"][0]["emotions"].is_object());
    assert!(r["results"][0]["quality"].is_number());
    assert!(r["reading"]["line"].as_str().unwrap().starts_with("idk"));
    assert!(r.get("hidden").is_none(), "no hidden counts");

    // legacy top-level limit/explain still work in v1; opts wins
    let r = d.ask(r#"{"op":"search","id":"s","q":"sad","limit":2,"explain":true}"#);
    assert_eq!((r["id"].as_str(), r["n"].as_u64()), (Some("s"), Some(2)));
    assert!(r["reading"].is_object());
    let r = d.ask(r#"{"op":"search","id":3,"q":"sad","limit":2,"opts":{"limit":4}}"#);
    assert_eq!(r["n"], 4);

    // unknown keys are warnings, bad values errors
    let r = d.ask(r#"{"op":"search","id":4,"q":"sad","foo":1,"opts":{"limit":1,"bar":2}}"#);
    assert_eq!(r["ok"], true);
    let codes: Vec<&str> = r["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"unknown_key"), "{r}");
    let r = d.ask(r#"{"op":"search","id":5,"q":"sad","opts":{"limit":"ten"}}"#);
    assert_eq!(r["ok"], false);
    assert_eq!(r["error"]["code"], "invalid_option");
    assert_eq!(r["error"]["key"], "opts.limit");
    let r = d.ask(r#"{"op":"frobnicate","id":6}"#);
    assert_eq!(r["error"]["code"], "unknown_op");
    let r = d.ask("{not json");
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("bad_json")));
    assert!(r["id"].is_null());

    // set_defaults applies under later requests
    let r = d.ask(r#"{"op":"set_defaults","id":7,"opts":{"limit":2,"safety":"moderate"}}"#);
    assert_eq!(r["ok"], true);
    let r = d.ask(r#"{"op":"search","id":8,"q":"happy"}"#);
    assert_eq!(r["n"], 2);
    let r = d.ask(r#"{"op":"set_defaults","id":9,"opts":{"safety":"spicy"}}"#);
    assert_eq!(r["error"]["code"], "invalid_option");

    // empty query: browse, canonical first
    let r = d.ask(r#"{"op":"search","id":10,"q":""}"#);
    assert_eq!(r["mode"], "browse");
    assert_eq!(r["results"][0]["from"], "canonical");

    let r = d.ask(r#"{"op":"search","id":11,"q":"happy","opts":{"limit":3}}"#);
    assert_eq!(r["counts"]["emoticon"], 3, "opts beats set_defaults: {r}");
    assert_eq!(r["n"], 3);
    // a request without an op is an error
    let r = d.ask(r#"{"id":4,"q":"sad","limit":25}"#);
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("bad_request")));

    // get, explain, similar, complete, info
    let r = d.ask(r#"{"op":"get","id":12,"ids":["k2026da3e4989","nope"]}"#);
    assert_eq!(r["entries"][0]["text"], "¯\\_(ツ)_/¯");
    assert!(r["entries"][1].is_null());
    let r = d.ask(r#"{"op":"explain","id":13,"q":"shy proud"}"#);
    assert!(r["reading"]["line"].as_str().unwrap().contains("proud"));
    let r = d.ask(r#"{"op":"similar","id":14,"face":"k2026da3e4989","opts":{"limit":3}}"#);
    assert_eq!((r["mode"].as_str(), r["n"].as_u64()), (Some("similar"), Some(3)));
    let r = d.ask(r#"{"op":"complete","id":15,"prefix":"emba","limit":3}"#);
    assert!(!r["completions"].as_array().unwrap().is_empty());
    let r = d.ask(r#"{"op":"info","id":16}"#);
    assert_eq!(r["protocol"], 1);
    assert_eq!(r["session_defaults"]["limit"], 2);
    assert!(r["state"]["blocklist"].as_str().unwrap().ends_with("emoticond/blocklist.txt"));
    d.send(r#"{"op":"quit"}"#);
    assert_eq!(d.finish(), 0);
}

#[test]
fn supersede_and_cancel_keep_responses_in_order() {
    let Some(repo) = data("supersede_and_cancel_keep_responses_in_order") else { return };
    let s = Sandbox::new(Some(&repo));
    let mut d = serve(&s);
    // all at once, so most are still queued when the first starts
    let mut batch = String::new();
    for (i, q) in ["s", "sa", "sad", "sad c", "sad ca", "sad cat"].iter().enumerate() {
        batch.push_str(&format!("{{\"op\":\"search\",\"id\":{i},\"q\":{q:?},\"chan\":\"main\",\"opts\":{{\"limit\":5}}}}\n"));
    }
    batch.push_str(r#"{"op":"search","id":"x","q":"happy","opts":{"limit":5}}"#);
    batch.push('\n');
    batch.push_str(r#"{"op":"cancel","target":"x"}"#);
    d.send(batch.trim_end());
    let mut got = Vec::new();
    for _ in 0..7 {
        got.push(serde_json::from_str::<Value>(&d.line()).unwrap());
    }
    let order: Vec<Value> = got.iter().map(|r| r["id"].clone()).collect();
    assert_eq!(order, [json!(0), json!(1), json!(2), json!(3), json!(4), json!(5), json!("x")], "one response each, in order");
    // the newest on the channel is always answered
    assert_eq!(got[5]["ok"], true);
    assert_eq!(got[5]["q"], "sad cat");
    for r in &got[..5] {
        assert!(r["ok"] == true || r["superseded"] == true, "{r}");
    }
    assert!(got[6]["ok"] == true || got[6]["cancelled"] == true);
    // nothing answers the cancel itself
    let r = d.ask(r#"{"op":"search","id":"after","q":"idk","opts":{"limit":1}}"#);
    assert_eq!(r["id"], "after");
}

#[test]
fn reports_from_both_menus_apply_and_share_state() {
    let Some(repo) = data("reports_from_both_menus_apply_and_share_state") else { return };
    let s = Sandbox::new(Some(&repo));
    let mut d = serve(&s);
    let mut other = serve(&s);
    let first = d.ask(r#"{"op":"search","id":1,"q":"sad","opts":{"limit":25,"explain":"reading"}}"#);
    let shown = ids(&first);
    let victim = shown[0].clone();
    let before_other = ids(&other.ask(r#"{"op":"search","id":1,"q":"sad","opts":{"limit":25}}"#));
    assert!(before_other.contains(&victim));

    // face menu: offensive hides at once, here and (after a refresh) elsewhere
    let ack = d.ask(&format!(r#"{{"op":"report","id":2,"about":1,"reason":"offensive","face":"{victim}"}}"#));
    assert_eq!(ack["ok"], true, "{ack}");
    assert_eq!(ack["queued"], true);
    assert_eq!(ack["sent"], false);
    assert_eq!(ack["sending"], true);
    assert_eq!(ack["effects"], json!(["blocked"]));
    assert!(ack["footer"].as_str().unwrap().contains("top 20"));
    let again = ids(&d.ask(r#"{"op":"search","id":3,"q":"sad","opts":{"limit":25}}"#));
    assert!(!again.contains(&victim));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let elsewhere = ids(&other.ask(r#"{"op":"search","id":2,"q":"sad","opts":{"limit":25}}"#));
    assert!(!elsewhere.contains(&victim), "the other process reloads the blocklist");
    // and in the CLI
    let o = s.run(&["search", "--json", "-n", "25", "sad"]);
    assert!(!ids(&serde_json::from_str(stdout(&o).trim()).unwrap()).contains(&victim));

    // the queued report carries the reading and the top 20 shown
    let q = s.read(s.state().join("reports/queue.jsonl"));
    let rep: Value = serde_json::from_str(q.lines().last().unwrap()).unwrap();
    assert_eq!(rep["reason"], "offensive");
    assert_eq!(rep["query"], "sad");
    assert_eq!(rep["shown"].as_array().unwrap().len(), 20);
    assert_eq!(rep["shown"][0], victim.as_str());
    assert!(rep["reading"].as_str().unwrap().contains("sad"));
    assert_eq!(rep["disclaimer_version"], 1);

    // clear: the Undo
    let ack = d.ask(&format!(r#"{{"op":"report","id":5,"about":1,"reason":"clear","face":"{victim}"}}"#));
    assert_eq!(ack["effects"], json!(["unblocked"]));
    assert!(ids(&d.ask(r#"{"op":"search","id":6,"q":"sad","opts":{"limit":25}}"#)).contains(&victim));

    // the other face reasons
    let ack = d.ask(&format!(r#"{{"op":"report","id":7,"about":1,"reason":"no_fit","face":"{}"}}"#, shown[1]));
    assert_eq!(ack["effects"], json!(["demoted"]));
    let ack = d.ask(&format!(r#"{{"op":"report","id":8,"about":1,"reason":"great_fit","face":"{}"}}"#, shown[2]));
    assert_eq!(ack["effects"], json!(["picked"]));
    let ack = d.ask(&format!(r#"{{"op":"report","id":9,"about":1,"reason":"other_word","face":"{}"}}"#, shown[3]));
    assert_eq!(ack["effects"], json!([]));
    let ack = d.ask(&format!(r#"{{"op":"report","id":10,"about":1,"reason":"note","face":"{}","note":"too sad"}}"#, shown[4]));
    assert_eq!(ack["ok"], true);

    // the query menu
    for (i, reason) in ["read_well", "read_wrong", "missing"].iter().enumerate() {
        let ack = d.ask(&format!(r#"{{"op":"report","id":{},"about":1,"reason":"{reason}"}}"#, 20 + i));
        assert_eq!(ack["ok"], true, "{ack}");
    }
    let ack = d.ask(r#"{"op":"report","id":30,"about":1,"reason":"note","note":"I meant blue"}"#);
    assert_eq!(ack["ok"], true);
    // without `about` (an idle restart): the explicit fields
    let ack = d.ask(r#"{"op":"report","id":31,"q":"angry","reason":"missing"}"#);
    assert_eq!(ack["ok"], true);
    let q = s.read(s.state().join("reports/queue.jsonl"));
    let rep: Value = serde_json::from_str(q.lines().last().unwrap()).unwrap();
    assert_eq!(rep["term_key"], "angry");
    assert_eq!(rep["shown"].as_array().unwrap().len(), 20);

    // bad reports
    assert_eq!(d.ask(r#"{"op":"report","id":40,"about":1,"reason":"offensive"}"#)["error"]["code"], "bad_request");
    assert_eq!(d.ask(r#"{"op":"report","id":41,"about":1,"reason":"great_fit_ish","face":"k2026da3e4989"}"#)["error"]["code"], "bad_request");
    assert_eq!(d.ask(r#"{"op":"report","id":42,"about":1,"reason":"note"}"#)["error"]["code"], "bad_request");
    assert_eq!(d.ask(r#"{"op":"report","id":43,"reason":"read_well"}"#)["error"]["code"], "bad_request");

    // usage was written by great_fit; blocklist and boosts exist
    assert!(s.read(s.state().join("usage.json")).contains(&shown[2]));
    assert!(s.read(s.state().join("overlays/boosts.jsonl")).contains(&shown[1][1..]));
}

#[test]
fn sending_off_is_reported_but_still_queued() {
    let Some(repo) = data("sending_off_is_reported_but_still_queued") else { return };
    let s = Sandbox::new(Some(&repo));
    s.config("[feedback]\nsend = false\n");
    let mut d = serve(&s);
    assert_eq!(d.ready["sending"], false);
    assert_eq!(d.ready["disclaimer"]["short"], "Reports are saved on this computer only.");
    d.ask(r#"{"op":"search","id":1,"q":"sad","opts":{"limit":5}}"#);
    let ack = d.ask(r#"{"op":"report","id":2,"about":1,"reason":"read_well"}"#);
    assert_eq!(ack["queued"], true);
    assert_eq!(ack["sending"], false);
    assert_eq!(ack["sending_off_by"], "user");
    assert_eq!(s.read(s.state().join("reports/queue.jsonl")).lines().count(), 1);
}

#[test]
fn picks_block_and_usage() {
    let Some(repo) = data("picks_block_and_usage") else { return };
    let s = Sandbox::new(Some(&repo));
    let mut d = serve(&s);
    let r = d.ask(r#"{"op":"search","id":1,"q":"idk","opts":{"limit":5}}"#);
    let pos = r["results"].as_array().unwrap().iter().position(|h| h["id"] == "k7fff199b7a51").expect("╮(╯_╰)╭ in the top 5 for idk") as u64;
    let ack = d.ask(r#"{"op":"pick","id":2,"about":1,"face":"k7fff199b7a51"}"#);
    assert_eq!((ack["recorded"].as_bool(), ack["logged"].as_bool()), (Some(true), Some(true)), "{ack}");
    let rec: Value = serde_json::from_str(s.read(s.picks()).trim()).unwrap();
    assert_eq!((rec["q"].as_str(), rec["rank"].as_u64()), (Some("idk"), Some(pos)), "the rank is where it was shown");
    assert_eq!(rec["shown"].as_array().unwrap().len(), 5);
    // a pick without an id gets no response; the next answer is the search
    d.send(r#"{"op":"pick","about":1,"face":"k7fff199b7a51"}"#);
    let r = d.ask(r#"{"op":"search","id":3,"q":"","opts":{"limit":300}}"#);
    let hist: Vec<&Value> = r["results"].as_array().unwrap().iter().filter(|h| h["from"] == "history").collect();
    assert_eq!(hist[0]["id"], "k7fff199b7a51", "history after the canonical faces");

    let ack = d.ask(r#"{"op":"block","id":4,"face":"k7fff199b7a51"}"#);
    assert_eq!(ack["changed"], true);
    assert!(!ids(&d.ask(r#"{"op":"search","id":5,"q":"idk","opts":{"limit":5}}"#)).contains(&"k7fff199b7a51".to_string()));
    let ack = d.ask(r#"{"op":"unblock","id":6,"face":"╮(╯_╰)╭"}"#);
    assert_eq!(ack["changed"], true);

    // a client-pushed usage map replaces the store's
    let ack = d.ask(r#"{"op":"usage","id":7,"map":{"global":{"k7fff199b7a51":0.0}}}"#);
    assert_eq!(ack["ok"], true);
    let r = d.ask(r#"{"op":"search","id":8,"q":"","opts":{"limit":300}}"#);
    assert!(r["results"].as_array().unwrap().iter().all(|h| h["from"] != "history" || h["id"] == "k7fff199b7a51"));
}

