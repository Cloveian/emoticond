mod common;

use common::{builder, face, report, clear, TempDir};
use emoticond::{FaceId, LocalEffect, Policy, PopularityMode, Reason, Report, Target};
use emoticond_state::{
    apply_local_effects, load_blocklists, read_boosts, Error, FeedbackQueue, LocalEffects, QueueLimits, SendError,
    SendOff, SendPolicy, Sender, StatePaths, UsageSettings, UsageStore,
};

const DAY: u64 = 86_400_000;

/// Accepts everything (or nothing), and remembers what it saw.
#[derive(Default)]
struct Mock {
    seen: Vec<String>,
    reject_all: bool,
    fail: bool,
}

impl Sender for Mock {
    fn send(&mut self, batch: &[Report]) -> Result<Vec<String>, SendError> {
        if self.fail {
            return Err(SendError::Transport("offline".into()));
        }
        self.seen.extend(batch.iter().map(|r| r.report_id.clone()));
        Ok(if self.reject_all { Vec::new() } else { batch.iter().map(|r| r.report_id.clone()).collect() })
    }
}

fn setup() -> (TempDir, StatePaths, FeedbackQueue) {
    let d = TempDir::new();
    let p = StatePaths::at(d.path());
    let q = FeedbackQueue::new(&p, QueueLimits::default());
    (d, p, q)
}

fn ids(q: &FeedbackQueue, now: u64) -> Vec<String> {
    q.pending(now).unwrap().into_iter().map(|r| r.report_id).collect()
}

#[test]
fn reports_carry_top_20_and_reading() {
    let (_d, _p, q) = setup();
    q.append(&report("r1", 1, "shrug", Target::Query, Reason::ReadWell), 1).unwrap();
    let p = q.pending(1).unwrap();
    let r = p.iter().next().unwrap();
    assert_eq!(r.shown.len(), 20);
    assert_eq!(r.reading.as_deref(), Some("shrug · test reading"));
    assert_eq!(r.disclaimer_version, emoticond_state::DISCLAIMER_VERSION);
}

#[test]
fn supersession_last_wins_per_key() {
    let (_d, _p, q) = setup();
    q.append(&report("a1", 1, "idk", face("(・_・)", 0), Reason::NoFit), 1).unwrap();
    q.append(&report("b1", 2, "idk", face("ಠ_ಠ", 1), Reason::NoFit), 2).unwrap();
    q.append(&report("a2", 3, " IDK ", face("(・_・)", 0), Reason::GreatFit), 3).unwrap();
    q.append(&report("c1", 4, "idk", Target::Query, Reason::Missing), 4).unwrap();
    assert_eq!(ids(&q, 5), ["b1", "a2", "c1"]);
}

#[test]
fn different_slots_stand_side_by_side() {
    let (_d, _p, q) = setup();
    // the query menu's verdict and "didn't return what I wanted" are separate
    q.append(&report("v", 1, "meh", Target::Query, Reason::ReadWell), 1).unwrap();
    q.append(&report("m", 2, "meh", Target::Query, Reason::Missing), 2).unwrap();
    // a face can be both "doesn't fit" and "offensive"
    q.append(&report("f", 3, "meh", face("(・_・)", 0), Reason::NoFit), 3).unwrap();
    q.append(&report("o", 4, "meh", face("(・_・)", 0), Reason::Offensive), 4).unwrap();
    assert_eq!(ids(&q, 5), ["v", "m", "f", "o"]);
    // clearing one slot leaves the other
    q.append(&clear("c", 5, "meh", Target::Query, Reason::Missing), 5).unwrap();
    q.append(&clear("d", 6, "meh", face("(・_・)", 0), Reason::Offensive), 6).unwrap();
    assert_eq!(ids(&q, 7), ["v", "f"]);
}

#[test]
fn clear_withdraws_unsent_and_is_sent_after_sent() {
    let (_d, _p, q) = setup();
    // never sent: Clear leaves nothing pending
    q.append(&report("a1", 1, "idk", face("(・_・)", 0), Reason::NoFit), 1).unwrap();
    q.append(&clear("a2", 2, "idk", face("(・_・)", 0), Reason::NoFit), 2).unwrap();
    assert!(q.pending(3).unwrap().is_empty());

    // sent, then cleared: the Clear must go out
    q.append(&report("b1", 3, "meh", Target::Query, Reason::ReadWrong), 3).unwrap();
    let mut m = Mock::default();
    let out = q.flush(&mut m, SendPolicy::Allowed, 4).unwrap();
    assert_eq!((out.sent, out.remaining), (1, 0));
    assert!(q.pending(5).unwrap().is_empty());
    q.append(&clear("b2", 5, "meh", Target::Query, Reason::ReadWrong), 5).unwrap();
    assert_eq!(ids(&q, 6), ["b2"]);
    q.flush(&mut m, SendPolicy::Allowed, 6).unwrap();
    assert!(q.pending(7).unwrap().is_empty());
    // a second Clear has nothing left to withdraw
    q.append(&clear("b3", 7, "meh", Target::Query, Reason::ReadWrong), 7).unwrap();
    assert!(q.pending(8).unwrap().is_empty());
    assert_eq!(m.seen, ["b1", "b2"]);
}

#[test]
fn flush_refuses_when_sending_is_off() {
    let (_d, _p, q) = setup();
    q.append(&report("a", 1, "idk", Target::Query, Reason::Missing), 1).unwrap();
    let mut m = Mock::default();
    let mut locked = Policy::default();
    locked.reports_disabled = true;
    // the packager's lock wins even when the user wants sending
    let e = q.flush(&mut m, SendPolicy::new(&locked, true), 2).unwrap_err();
    assert!(matches!(e, Error::SendingOff(SendOff::Policy)));
    assert!(e.is_sending_locked());
    let e = q.flush(&mut m, SendPolicy::Off(SendOff::User), 2).unwrap_err();
    assert!(matches!(e, Error::SendingOff(SendOff::User)));
    #[cfg(not(feature = "net"))]
    assert!(matches!(
        q.flush(&mut m, SendPolicy::new(&Policy::default(), true), 2),
        Err(Error::SendingOff(SendOff::NotBuilt))
    ));
    assert!(m.seen.is_empty());
    // still queued
    assert_eq!(ids(&q, 3), ["a"]);
}

#[test]
fn failed_or_rejected_sends_stay_queued() {
    let (_d, _p, q) = setup();
    q.append(&report("a", 1, "idk", Target::Query, Reason::Missing), 1).unwrap();
    let mut m = Mock { fail: true, ..Mock::default() };
    assert!(matches!(q.flush(&mut m, SendPolicy::Allowed, 2), Err(Error::Send(SendError::Transport(_)))));
    let mut m = Mock { reject_all: true, ..Mock::default() };
    let out = q.flush(&mut m, SendPolicy::Allowed, 2).unwrap();
    assert_eq!((out.sent, out.remaining), (0, 1));
    assert_eq!(ids(&q, 3), ["a"]);
}

#[test]
fn queue_limits_and_compaction() {
    let d = TempDir::new();
    let p = StatePaths::at(d.path());
    let mut lim = QueueLimits::default();
    lim.queue_max = 3;
    lim.max_age_days = 10;
    let q = FeedbackQueue::new(&p, lim);
    for i in 0..5u64 {
        let r = report(&format!("r{i}"), i * DAY, &format!("q{i}"), Target::Query, Reason::Missing);
        q.append(&r, i * DAY).unwrap();
    }
    // newest three kept
    assert_eq!(ids(&q, 5 * DAY), ["r2", "r3", "r4"]);
    // r2 is older than 10 days at day 12.5
    assert_eq!(ids(&q, 12 * DAY + DAY / 2), ["r3", "r4"]);
    // a bad line is a warning, not an error
    std::fs::OpenOptions::new()
        .append(true)
        .open(&p.queue)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"{oops\n"))
        .unwrap();
    assert_eq!(q.pending(5 * DAY).unwrap().warnings.len(), 1);
    q.compact(5 * DAY).unwrap();
    let text = std::fs::read_to_string(&p.queue).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert_eq!(ids(&q, 5 * DAY), ["r2", "r3", "r4"]);
}

#[test]
fn append_compacts_a_long_queue() {
    let d = TempDir::new();
    let p = StatePaths::at(d.path());
    let mut lim = QueueLimits::default();
    lim.queue_max = 1; // compaction threshold is max(queue_max, 32) * 2 lines
    let q = FeedbackQueue::new(&p, lim);
    for i in 0..70u64 {
        q.append(&report(&format!("r{i}"), i, "same", Target::Query, Reason::Missing), i).unwrap();
    }
    let lines = std::fs::read_to_string(&p.queue).unwrap().lines().count();
    assert!(lines < 65, "{lines}");
    assert_eq!(ids(&q, 70), ["r69"]);
}

#[test]
fn local_effects_apply() {
    let (_d, p, _q) = setup();
    let mut usage = UsageStore::open_in(&p, UsageSettings::default());
    let bad = FaceId::of_text("(╬ Ò﹏Ó)");
    let good = FaceId::of_text("(^‿^)");
    let meh = FaceId::of_text("(・_・)");
    let fx = vec![
        LocalEffect::RecordPick { id: good, term_key: Some("happy".into()) },
        LocalEffect::Demote { id: meh, term_key: "happy".into() },
        LocalEffect::Block { id: bad, text: Some("(bad face)".into()) },
    ];
    let a = apply_local_effects(&p, Some(&mut usage), &fx, true, 1000).unwrap();
    assert_eq!(a.blocked, [bad]);
    assert_eq!(a.picked, [good]);
    assert_eq!(a.demoted, [("happy".to_string(), meh)]);
    assert!(a.skipped.is_empty());
    assert!(load_blocklists(&[&p.blocklist]).ids.contains(&bad));
    assert_eq!(usage.map(1000).weight(good, Some("happy")), 0.5);
    let (rows, _) = read_boosts(&p.boosts).unwrap();
    assert_eq!((rows[0].term.as_str(), rows[0].id, rows[0].boost), ("happy", meh, -3));
}

#[test]
fn apply_locally_off_still_hides_offensive() {
    let (_d, p, _q) = setup();
    let mut usage = UsageStore::open_in(&p, UsageSettings::default());
    let bad = FaceId::of_text("bad");
    let fx = vec![
        LocalEffect::RecordPick { id: FaceId::of_text("good"), term_key: None },
        LocalEffect::Block { id: bad, text: Some("(bad face)".into()) },
        LocalEffect::Demote { id: FaceId::of_text("meh"), term_key: "x".into() },
    ];
    let a = apply_local_effects(&p, Some(&mut usage), &fx, false, 1).unwrap();
    assert_eq!(a.blocked, [bad]);
    assert_eq!(a.skipped.len(), 2);
    assert!(usage.state().is_empty());
    assert!(!p.boosts.exists());
}

#[test]
fn popularity_off_skips_picks() {
    let (_d, p, _q) = setup();
    let mut s = UsageSettings::default();
    s.mode = PopularityMode::Off;
    let mut usage = UsageStore::open_in(&p, s);
    let fx = [LocalEffect::RecordPick { id: FaceId::of_text("good"), term_key: None }];
    let a = apply_local_effects(&p, Some(&mut usage), &fx, true, 1).unwrap();
    assert!(a.picked.is_empty());
    assert_eq!(a.skipped.len(), 1);
    assert!(!p.usage.exists());
}

#[test]
fn offensive_submit_hides_at_once_even_when_sending_is_locked() {
    let (_d, p, q) = setup();
    let b = builder("cute", face("(╬ Ò﹏Ó)", 4), Reason::Offensive);
    let fx = b.local_effects();
    let r = b.stamp("off1", 10, emoticond_state::DISCLAIMER_VERSION).build().unwrap();
    let mut local = LocalEffects::new(&p, None, false);
    let applied = q.submit(&r, &fx, &mut local, 10).unwrap();
    assert_eq!(applied.blocked, [FaceId::of_text("(╬ Ò﹏Ó)")]);
    assert!(load_blocklists(&p.blocklist_sources()).ids.contains(&FaceId::of_text("(╬ Ò﹏Ó)")));
    // queued, but the locked policy refuses to send it
    assert_eq!(ids(&q, 11), ["off1"]);
    let mut locked = Policy::default();
    locked.reports_disabled = true;
    assert!(q.flush(&mut Mock::default(), SendPolicy::new(&locked, true), 11).unwrap_err().is_sending_locked());
}
