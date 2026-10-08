//! Loading, precedence, policy, env aliases, warnings and the sample files
//! from docs/options.md.

use emoticond::{Dataset, Explain, FaceId, Figures, PopularityMode, Safety, SearchOptions, StyleMode, UsageWeight};
use emoticond_config::{codes, level, ConfigError, Env, Level, Loader, Paths, Source, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const SAMPLE_CONFIG: &str = include_str!("fixtures/config.toml");
const OPTIONS_MD_CONFIG: &str = include_str!("fixtures/options-md-config.toml");
const SAMPLE_POLICY: &str = include_str!("fixtures/policy.toml");

/// A fresh, empty directory per test.
fn tmp() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("emoticond-config-tests")
        .join(format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn codes_of(w: &[emoticond::Warning]) -> Vec<String> {
    w.iter().map(|w| w.code.to_string()).collect()
}

#[test]
fn defaults_are_the_stranger_defaults() {
    // options.md §9.
    let root = tmp();
    let r = Loader::isolated(&root).load().unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let s = &r.search;
    assert_eq!(*s, {
        let mut d = SearchOptions::default();
        d.usage_weight = UsageWeight::Normal;
        d
    });
    assert_eq!(s.safety, Safety::Strict);
    assert_eq!((s.styles.lenny, s.styles.crude, s.styles.long), (StyleMode::Demote, StyleMode::Hide, StyleMode::Demote));
    assert_eq!(s.long_at, 14);
    assert_eq!(s.figures, Figures::Auto);
    assert!(s.pinned);
    assert_eq!(s.min_quality, None);
    assert_eq!(s.dedupe.threshold(), 0.55);
    assert_eq!(s.variety, 0.0);
    assert_eq!(s.limit, 40);
    assert_eq!(s.explain, Explain::Off);
    let c = &r.config;
    assert_eq!(c.popularity.mode, PopularityMode::Local);
    assert_eq!(c.popularity.half_life_days, 30.0);
    assert_eq!(c.popularity.max_entries, 5000);
    assert!(c.popularity.remember_terms);
    assert!(c.feedback.send && c.feedback.menus && c.feedback.apply_locally);
    // reports: sent unless turned off; usage stats: asked, nothing sent until yes
    assert_eq!(r.reports_choice(), emoticond_config::Choice::Default(true));
    assert!(r.send_reports());
    assert_eq!(r.stats_choice(), emoticond_config::Choice::Unasked);
    assert_eq!(r.popularity_mode(), PopularityMode::Local);
    assert_eq!(c.popularity.share_endpoint.as_deref(), Some(emoticond_config::DEFAULT_STATS_ENDPOINT));
    assert_eq!((c.feedback.queue_max, c.feedback.queue_max_age_days, c.feedback.custom_max_chars), (500, 180, 500));
    assert_eq!(c.feedback.endpoint.as_deref(), Some(emoticond_config::DEFAULT_REPORT_ENDPOINT));
    assert_eq!(c.dev.pick_log, None);
    assert_eq!(c.daemon.idle_exit, 120);
    assert_eq!(c.data.dataset, Dataset::Core);
    assert!(r.policy.is_open() && r.locks.is_empty());
    // Paths filled in.
    let p = Paths::rooted(&root);
    assert_eq!(c.popularity.store.as_deref(), Some(p.usage_file().as_path()));
    assert_eq!(c.feedback.queue.as_deref(), Some(p.reports_queue().as_path()));
    let open = r.open_options();
    assert_eq!(open.data_dirs, p.data_dirs);
    assert_eq!(open.dataset, Dataset::Core);
    assert!(open.overlays.is_empty(), "overlay dirs only count when they exist");
    assert_eq!(open.defaults, r.search);
    assert_eq!(r.blocklist_files, [p.state_blocklist(), p.system_blocklist.clone()]);
    for d in r.describe().settings {
        assert_eq!(d.source, Source::Default, "{}", d.key);
    }
}

#[test]
fn sample_config_parses_cleanly() {
    let root = tmp();
    let r = Loader::isolated(&root).user_toml(SAMPLE_CONFIG).load().unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let home = root.join("home");
    assert_eq!(r.search.safety, Safety::Moderate);
    assert_eq!(r.search.limit, 50);
    assert_eq!(r.search.styles.crude, StyleMode::Demote);
    assert_eq!(r.search.explain, Explain::Reading, "from ui.show_reading");
    assert_eq!(r.source("explain"), Some(&Source::Derived("ui.show_reading")));
    assert_eq!(r.config.popularity.half_life_days, 60.0);
    assert_eq!(r.config.ui.max_results, Some(100));
    assert_eq!(r.config.data.dataset, Dataset::Full);
    assert_eq!(r.config.dev.pick_log, Some(home.join("src/emoticond/picks.jsonl")));
    assert_eq!(r.data_dirs[0], home.join("src/emoticond/data-dev"), "config dirs first");
    assert_eq!(&r.data_dirs[1..], Paths::rooted(&root).data_dirs);

    let cli = Loader::isolated(&root).user_toml(SAMPLE_CONFIG).frontend("cli").load().unwrap();
    assert_eq!(cli.profile.as_deref(), Some("cli"));
    assert!(!cli.search.complete_partial);
    assert_eq!(cli.search.explain, Explain::Off, "an explicit explain beats ui.show_reading");
    let qs = Loader::isolated(&root).user_toml(SAMPLE_CONFIG).frontend("quickshell").load().unwrap();
    assert_eq!(qs.search.limit, 100);
    assert!(qs.search.complete_partial);
}

#[test]
fn options_md_sample_config_loads_without_warnings() {
    // the verbatim options.md §7.4 sample
    let root = tmp();
    let r = Loader::isolated(&root).user_toml(OPTIONS_MD_CONFIG).frontend("cli").load().unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.search.limit, 50);
    assert_eq!(r.search.safety, Safety::Moderate);
    assert!(!r.search.complete_partial);
}

#[test]
fn sample_policy_clamps_and_locks() {
    let root = tmp();
    write(&Paths::rooted(&root).policy_file, SAMPLE_POLICY);
    let r = Loader::isolated(&root).user_toml(SAMPLE_CONFIG).load().unwrap();
    let errors: Vec<_> = r.warnings.iter().filter(|w| level(w) == Level::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    // Ceiling: the sample's moderate is clamped to strict.
    assert_eq!(r.search.safety, Safety::Strict);
    assert!(matches!(r.source("safety"), Some(Source::PolicyCeiling(_))));
    // Locks: crude hide, feedback.send false, dataset core.
    assert_eq!(r.search.styles.crude, StyleMode::Hide);
    assert!(!r.config.feedback.send && !r.send_reports());
    assert!(r.policy.reports_disabled);
    assert_eq!(r.config.data.dataset, Dataset::Core);
    assert_eq!(r.policy.max_popularity, Some(PopularityMode::Local));
    assert!(r.locks.locked("feedback.send") && r.locks.locked("styles.crude") && r.locks.locked("data.dataset"));
    assert!(!r.locks.locked("styles.lenny") && !r.locks.locked("safety"));
    assert!(r.locks.allows("safety", "strict") && !r.locks.allows("safety", "off"));
    // The user is told, at info level.
    let info: Vec<_> = r.warnings.iter().filter(|w| level(w) == Level::Info).filter_map(|w| w.key.clone()).collect();
    assert_eq!(info, ["data.dataset", "feedback.send", "search.styles.crude", "search.safety"]);
    // The policy also travels to the library.
    let open = r.open_options();
    assert_eq!(open.policy, r.policy);
    assert_eq!(open.defaults.safety, Safety::Strict);
    assert!(r.blocklist_files.contains(&PathBuf::from("/etc/emoticond/blocklist.txt")));
    let shown = r.describe().to_string();
    assert!(shown.contains("feedback.send = false"), "{shown}");
    assert!(shown.contains("[locked]"), "{shown}");
}

/// Write one layer of `search.limit` per precedence level and check that
/// removing the top one each time reveals the next.
#[test]
fn each_layer_beats_the_next() {
    let root = tmp();
    let p = Paths::rooted(&root);
    let build = |levels: &[u8]| {
        let mut sys = String::new();
        let mut user = String::new();
        if levels.contains(&1) {
            sys += "[search]\nlimit = 11\n";
        }
        if levels.contains(&2) {
            sys += "[profile.picker.search]\nlimit = 12\n";
        }
        if levels.contains(&3) {
            user += "[search]\nlimit = 13\n";
        }
        if levels.contains(&4) {
            user += "[profile.picker.search]\nlimit = 14\n";
        }
        let mut l = Loader::isolated(&root).frontend("picker").system_toml(sys).user_toml(user);
        if levels.contains(&7) {
            l = l.set("limit=17");
        }
        if levels.contains(&8) {
            l = l.policy_toml("[locks]\nsearch.limit = 18\n");
        }
        let r = l.load().unwrap();
        if levels.contains(&5) {
            r.with_request(&serde_json::json!({"limit": 15})).unwrap().options.limit
        } else {
            r.search.limit
        }
    };
    let all = [1, 2, 3, 4, 5, 7, 8];
    for top in (0..all.len()).rev() {
        let expect = u16::from(all[top]) + 10;
        assert_eq!(build(&all[..=top]), expect, "layers {:?}", &all[..=top]);
    }
    assert_eq!(build(&[]), 40, "defaults");

    // Env sits between the request and the CLI; use a key env can set.
    let env = Env::empty().with("EMOTICOND_PICK_LOG", "/env.jsonl");
    let user = "[dev]\npick_log = \"/user.jsonl\"\n";
    let load = |cli: bool| {
        let mut l = Loader::isolated(&root).env(env.clone()).user_toml(user);
        if cli {
            l = l.set("dev.pick_log=/cli.jsonl");
        }
        l.load().unwrap()
    };
    assert_eq!(load(false).config.dev.pick_log, Some(PathBuf::from("/env.jsonl")));
    assert_eq!(load(false).source("dev.pick_log"), Some(&Source::Env("EMOTICOND_PICK_LOG")));
    assert_eq!(load(true).config.dev.pick_log, Some(PathBuf::from("/cli.jsonl")));
    assert_eq!(load(true).source("dev.pick_log"), Some(&Source::Cli));
    let _ = p;
}

#[test]
fn user_config_beats_system_profile_order_and_system_dirs_priority() {
    let root = tmp();
    let mut p = Paths::rooted(&root);
    p.config_dirs = vec![root.join("sys-a"), root.join("sys-b")];
    write(&root.join("sys-a/config.toml"), "[search]\ndedupe = \"strong\"\n");
    write(&root.join("sys-b/config.toml"), "[search]\ndedupe = \"off\"\nfigures = \"pair\"\n");
    let r = Loader::isolated(&root).paths(p.clone()).load().unwrap();
    assert_eq!(r.search.dedupe, emoticond::Dedupe::Strong, "the first XDG_CONFIG_DIRS entry wins");
    assert_eq!(r.search.figures, Figures::Pair, "lower dirs still fill gaps");
    assert_eq!(r.files, [root.join("sys-b/config.toml"), root.join("sys-a/config.toml")]);
    write(&p.user_config(), "[search]\ndedupe = \"normal\"\n");
    let r = Loader::isolated(&root).paths(p).load().unwrap();
    assert_eq!(r.search.dedupe, emoticond::Dedupe::Normal);
    assert!(matches!(r.source("dedupe"), Some(Source::UserConfig(_))));
}

#[test]
fn profile_selection() {
    let root = tmp();
    let user = "[search]\nlimit = 1\n[profile.a.search]\nlimit = 2\n[profile.b.search]\nlimit = 3\n";
    let l = || Loader::isolated(&root).user_toml(user);
    assert_eq!(l().load().unwrap().search.limit, 1);
    assert_eq!(l().frontend("a").load().unwrap().search.limit, 2);
    let env = Env::empty().with("EMOTICOND_PROFILE", "b");
    assert_eq!(l().frontend("a").env(env.clone()).load().unwrap().search.limit, 3, "env beats the front-end name");
    assert_eq!(l().frontend("a").env(env).profile("a").load().unwrap().search.limit, 2, "--profile beats env");
    // A front-end without a profile is normal; an explicit missing one warns.
    assert!(l().frontend("nope").load().unwrap().warnings.is_empty());
    let r = l().profile("nope").load().unwrap();
    assert_eq!(codes_of(&r.warnings), [codes::UNKNOWN_PROFILE]);
    assert_eq!(r.search.limit, 1);
}

#[test]
fn env_aliases_new_and_legacy() {
    let root = tmp();
    let load = |env: Env| Loader::isolated(&root).env(env).load().unwrap();
    let dflt = Paths::rooted(&root).data_dirs;

    let r = load(Env::empty().with("EMOTICOND_DATA", "/a:/b"));
    assert_eq!(r.data_dirs[..2], [PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(r.data_dirs[2..], dflt[..]);
    assert!(r.warnings.is_empty());

    let r = load(
        Env::empty()
            .with("EMOTICOND_PICK_LOG", "~/p")
            .with("EMOTICOND_IDLE_EXIT", "30")
            .with("EMOTICOND_TUNING_WEIGHTS", "0.5,1,1,0.1"),
    );
    assert_eq!(r.config.dev.pick_log, Some(root.join("home/p")), "~ is expanded");
    assert_eq!(r.config.daemon.idle_exit, 30);
    assert_eq!(r.config.tuning_weights, Some([0.5, 1.0, 1.0, 0.1]));
    assert_eq!(r.source("dev.pick_log"), Some(&Source::Env("EMOTICOND_PICK_LOG")));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    let r = load(Env::empty().with("EMOTICOND_IDLE_EXIT", "soon"));
    assert_eq!(r.config.daemon.idle_exit, 120);
    assert!(codes_of(&r.warnings).contains(&codes::INVALID_VALUE.to_string()));
}

#[test]
fn emoticond_config_env_and_cli_choose_files() {
    let root = tmp();
    let p = Paths::rooted(&root);
    write(&p.user_config(), "[search]\nlimit = 7\n");
    write(&root.join("other.toml"), "[search]\nlimit = 8\n");
    let env = |v: &str| Env::empty().with("EMOTICOND_CONFIG", v.to_string());
    assert_eq!(Loader::isolated(&root).load().unwrap().search.limit, 7);
    assert_eq!(Loader::isolated(&root).env(env("none")).load().unwrap().search.limit, 40);
    let other = root.join("other.toml");
    assert_eq!(Loader::isolated(&root).env(env(other.to_str().unwrap())).load().unwrap().search.limit, 8);
    assert_eq!(Loader::isolated(&root).config_file(&other).load().unwrap().search.limit, 8);
    assert_eq!(Loader::isolated(&root).env(env(other.to_str().unwrap())).no_config().load().unwrap().search.limit, 40);
    // A missing --config is an error; a missing $EMOTICOND_CONFIG a warning.
    assert!(matches!(Loader::isolated(&root).config_file(root.join("missing.toml")).load(), Err(ConfigError::Io { .. })));
    let r = Loader::isolated(&root).env(env(root.join("missing.toml").to_str().unwrap())).load().unwrap();
    assert_eq!(codes_of(&r.warnings), [codes::CONFIG_UNREADABLE]);
}

#[test]
fn bad_files_give_warnings_not_errors() {
    let root = tmp();
    let text = r#"
foo = 1
[search]
bogus = true
safety = "spicy"
limit = "ten"
long_at = 0
variety = 3
max_len = "none"
[search.emotions.target]
sad = 2.0
[search.styles]
lenny = 1
[popularity]
weight = "low"
[feedback]
endpoint = "ftp://nope"
"#;
    let r = Loader::isolated(&root).system_toml("[search]\nlimit = 9\nsafety = \"moderate\"\n").user_toml(text).load().unwrap();
    let mut got: Vec<_> = r.warnings.iter().map(|w| format!("{} {}", w.code, w.key.as_deref().unwrap_or("-"))).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "invalid_value feedback.endpoint",
            "invalid_value search.limit",
            "invalid_value search.safety",
            "invalid_value search.styles.lenny",
            "out_of_range search.emotions.target",
            "out_of_range search.long_at",
            "out_of_range search.variety",
            "unknown_key foo",
            "unknown_key search.bogus",
        ]
    );
    assert_eq!(r.search.safety, Safety::Strict, "unknown enum value: the default, not the system's moderate");
    assert_eq!(r.search.limit, 9, "a type error is ignored, so the lower layer stands");
    assert_eq!(r.search.long_at, 1);
    assert_eq!(r.search.variety, 1.0);
    assert_eq!(r.search.emotions.target["sad"], 1.0);
    assert_eq!(r.search.usage_weight, UsageWeight::Low);
    assert!(r.warnings.iter().all(|w| level(w) == Level::Warning));

    let r = Loader::isolated(&root).user_toml("[search\nlimit = 3").load().unwrap();
    assert_eq!(codes_of(&r.warnings), [codes::CONFIG_UNREADABLE]);
    assert_eq!(r.search.limit, 40);

    let r = Loader::isolated(&root).user_toml("config_version = 2\n[search]\nlimit = 3\n").load().unwrap();
    assert_eq!(codes_of(&r.warnings), [codes::NEWER_VERSION]);
    assert_eq!(r.search.limit, 3);
}

#[test]
fn cli_overrides_fail_loudly() {
    let root = tmp();
    let l = || Loader::isolated(&root);
    assert!(matches!(l().set("bogus=1").load(), Err(ConfigError::UnknownKey { .. })));
    assert!(matches!(l().set("safety=spicy").load(), Err(ConfigError::InvalidValue { .. })));
    assert!(matches!(l().set("limit=ten").load(), Err(ConfigError::InvalidValue { .. })));
    assert!(matches!(l().set("safety").load(), Err(ConfigError::Syntax(_))));
    let r = l().set_all(["safety=off", "styles.lenny=hide", "min-quality=4.5", "max_len=12", "limit=900", "emotions.max=angry:0.2"]).load().unwrap();
    assert_eq!(r.search.safety, Safety::Off);
    assert_eq!(r.search.styles.lenny, StyleMode::Hide);
    assert_eq!(r.search.min_quality, Some(4.5));
    assert_eq!(r.search.max_len, Some(12));
    assert_eq!(r.search.limit, 500);
    assert_eq!(r.search.emotions.max["angry"], 0.2);
    assert_eq!(codes_of(&r.warnings), [codes::OUT_OF_RANGE]);
    let r = l().user_toml("[search]\nmax_len = 10\n").set("max_len=none").load().unwrap();
    assert_eq!(r.search.max_len, None);
}

#[test]
fn daemon_requests() {
    let root = tmp();
    let r = Loader::isolated(&root).policy_toml("[ceilings]\nsafety.min = \"moderate\"\n").user_toml("[search]\nlimit = 30\n").load().unwrap();
    let id = FaceId::of_text("(._.)");
    let q = r
        .with_request(&serde_json::json!({
            "safety": "off", "offset": 10, "seed": 7, "exclude": [id.to_string()],
            "styles": {"lenny": "hide"}, "kinds": ["emoticon"], "nonsense": 1, "max_len": null
        }))
        .unwrap();
    assert_eq!(q.options.limit, 30, "config below the request still applies");
    assert_eq!(q.options.safety, Safety::Moderate, "clamped by the ceiling");
    assert_eq!(q.options.styles.lenny, StyleMode::Hide);
    assert_eq!((q.options.offset, q.options.seed), (10, 7));
    assert!(q.options.exclude.contains(&id));
    let mut c = codes_of(&q.warnings);
    c.sort();
    assert_eq!(c, [codes::CLAMPED, codes::UNKNOWN_KEY, codes::UNKNOWN_KEY]);
    assert_eq!(r.search.safety, Safety::Strict, "the request doesn't change the loaded config");
    assert!(matches!(r.with_request(&serde_json::json!({"limit": "ten"})), Err(ConfigError::InvalidValue { .. })));
    assert!(matches!(r.with_request(&serde_json::json!({"safety": "spicy"})), Err(ConfigError::InvalidValue { .. })));
    assert!(matches!(r.with_request(&serde_json::json!({"exclude": ["xyz"]})), Err(ConfigError::InvalidValue { .. })));
}

#[test]
fn policy_locks_beat_cli_and_popularity_ceiling_off() {
    let root = tmp();
    let r = Loader::isolated(&root)
        .policy_toml("[locks]\nsearch.safety = \"strict\"\nsearch.dedupe = \"strong\"\n[ceilings]\npopularity.max = \"off\"\n")
        .user_toml("[popularity]\nmode = \"shared\"\n")
        .set("safety=off")
        .set("dedupe=off")
        .load()
        .unwrap();
    assert_eq!(r.search.safety, Safety::Strict);
    assert_eq!(r.search.dedupe, emoticond::Dedupe::Strong, "config-level locks work for any key");
    assert!(r.locks.locked("dedupe") && r.locks.locked("safety"));
    assert_eq!(r.locks.value("dedupe"), Some(&Value::Str("strong".into())));
    assert_eq!(r.popularity_mode(), PopularityMode::Off);
    assert_eq!(r.search.usage_weight, UsageWeight::Off);
    assert!(!r.locks.allows("popularity.mode", "local") && r.locks.allows("popularity.mode", "off"));
    assert_eq!(r.policy.safety, Some(Safety::Strict));
}

#[test]
fn users_can_turn_sending_off_themselves() {
    // options.md §5.2: both the user and the packager.
    let root = tmp();
    let r = Loader::isolated(&root).user_toml("[feedback]\nsend = false\n").load().unwrap();
    assert!(!r.send_reports());
    assert!(!r.policy.reports_disabled, "a user choice, not a policy");
    assert!(!r.locks.locked("feedback.send"));
}

#[test]
fn unreadable_policy_fails_closed() {
    let root = tmp();
    let r = Loader::isolated(&root).policy_toml("[ceilings\n").set("safety=off").load().unwrap();
    assert_eq!(r.search.safety, Safety::Strict);
    assert!(!r.send_reports());
    assert!(r.warnings.iter().any(|w| w.code == codes::POLICY_UNREADABLE && level(w) == Level::Error));
}

#[test]
fn paths_in_files_resolve_against_the_file_and_home() {
    let root = tmp();
    let p = Paths::rooted(&root);
    write(&p.user_config(), "[data]\ndirs = [\"rel\", \"~/abs\"]\noverlays = [\"o\"]\n");
    std::fs::create_dir_all(p.user_overlays_dir()).unwrap();
    let r = Loader::isolated(&root).load().unwrap();
    assert_eq!(r.config.data.dirs, [p.config_home.join("rel"), root.join("home/abs")]);
    assert_eq!(r.overlays, [p.user_overlays_dir(), p.config_home.join("o")], "existing overlay dirs first");
}

#[test]
fn describe_names_every_source() {
    let root = tmp();
    let r = Loader::isolated(&root)
        .env(Env::empty().with("EMOTICOND_IDLE_EXIT", "9"))
        .user_toml("[search]\nsafety = \"moderate\"\n")
        .set("figures=pair")
        .load()
        .unwrap();
    let d = r.describe();
    let find = |k: &str| d.settings.iter().find(|s| s.key == k).unwrap();
    assert!(matches!(find("search.safety").source, Source::UserConfig(_)));
    assert_eq!(find("daemon.idle_exit").source, Source::Env("EMOTICOND_IDLE_EXIT"));
    assert_eq!(find("search.figures").source, Source::Cli);
    assert_eq!(find("search.pinned").source, Source::Default);
    let text = d.to_string();
    assert!(text.contains("search.safety = \"moderate\""), "{text}");
    assert!(text.contains("# env EMOTICOND_IDLE_EXIT"), "{text}");
    assert!(text.contains("# command line"), "{text}");
    assert!(text.lines().count() > emoticond_config::settings().count());
}

#[test]
fn settings_registry_is_complete_for_a_settings_page() {
    let basic: Vec<_> = emoticond_config::settings().filter(|s| s.page == emoticond_config::Page::Basic).map(|s| s.key).collect();
    // options.md §8.1.
    assert_eq!(basic, ["search.safety", "search.styles.lenny", "search.styles.crude", "popularity.mode"]);
    let s = emoticond_config::setting("popularity.mode").unwrap();
    assert_eq!(s.choices, ["off", "local", "shared"]);
    assert_eq!(s.default, Value::Str("local".into()));
}

#[test]
fn tuning_weights_follow_the_feature() {
    let root = tmp();
    let r = Loader::isolated(&root).user_toml("[advanced.tuning]\nweights = { emotion = 1.0, lexical = 0.0 }\n").load().unwrap();
    assert_eq!(r.config.tuning_weights, Some([1.0, 1.2, 1.2, 0.0]));
    #[cfg(feature = "unstable-tuning")]
    {
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.search.tuning.as_ref().and_then(|t| t.weights), Some([1.0, 1.2, 1.2, 0.0]));
    }
    #[cfg(not(feature = "unstable-tuning"))]
    assert_eq!(codes_of(&r.warnings), [codes::TUNING_IGNORED]);
}
