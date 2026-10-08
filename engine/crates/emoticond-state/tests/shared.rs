//! Two front-ends on one state dir: what one writes, the other sees on its
//! next refresh.

mod common;

use common::TempDir;
use emoticond::{FaceId, Pick, UsageState};
use emoticond_state::{add_to_blocklist, demote, import_picks, normalize_query, Shared, StatePaths, UsageSettings};

fn open(p: &StatePaths) -> Shared {
    let mut s = UsageSettings::default();
    s.refresh_interval_ms = 1000;
    Shared::open(p, s, &p.blocklist_sources(), &[&p.boosts])
}

#[test]
fn a_report_in_one_process_shows_in_the_other_after_refresh() {
    let d = TempDir::new();
    let p = StatePaths::at(d.path());
    let mut picker = open(&p);
    let mut krunner = open(&p);
    assert!(!picker.refresh(0).any());

    let bad = FaceId::of_text("(╬ Ò﹏Ó)");
    add_to_blocklist(&p.blocklist, bad).unwrap();
    krunner.usage.record(&Pick::new(FaceId::of_text("(^‿^)"), Some("happy".into())), 100).unwrap();
    demote(&p.boosts, "happy", FaceId::of_text("(・_・)")).unwrap();

    // within the interval nothing is re-stat'ed
    assert!(!picker.refresh(500).any());
    assert!(!picker.blocklist().ids.contains(&bad));
    let c = picker.refresh(1000);
    assert!(c.usage && c.blocklist && c.overlays);
    assert!(picker.blocklist().ids.contains(&bad));
    assert_eq!(picker.usage_map(100).weight(FaceId::of_text("(^‿^)"), Some("happy")), 0.5);
    assert_eq!(picker.overlay_paths(), std::slice::from_ref(&p.boosts));
    // the writer does not reload its own write
    assert!(!krunner.refresh(2000).usage);
}

#[test]
fn import_into_the_store() {
    let d = TempDir::new();
    let p = StatePaths::at(d.path());
    let picks = d.path().join("picks.jsonl");
    std::fs::write(
        &picks,
        "{\"kind\":\"emoticon\",\"q\":\"idk\",\"rank\":0,\"shown\":[],\"text\":\"¯\\\\_(ツ)_/¯\",\"ts\":1791169005.6}\n",
    )
    .unwrap();
    let mut sh = open(&p);
    let mut summary = None;
    sh.usage
        .update(0, |st: &mut UsageState| summary = Some(import_picks(&picks, st, normalize_query).unwrap()))
        .unwrap();
    assert_eq!(summary.unwrap().imported, 1);
    let shrug = FaceId::of_text("¯\\_(ツ)_/¯");
    assert_eq!(sh.usage_map(1_791_169_005_600).weight(shrug, Some("idk")), 0.5);
}
