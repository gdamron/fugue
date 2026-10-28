//! Outcomes the audio thread drops when no control thread has made room:
//! an untimed write is taken to have applied, as most do. A timed one's
//! outcome has room kept for it, so it stays listed, unrecorded, until its
//! own outcome settles it, or, with a ttl, until the audio thread renders
//! past it.

use std::sync::Arc;

use crate::control_request::{
    outcome_channel_reserving, Outcome, Refusal, RequestId, Source, Transport, When,
};
use crate::invention::publish::{Authoring, DocumentStep, PendingLog, PendingWrite};
use crate::ControlValue;

/// An authored, announced write of `osc.frequency`, made at `when`.
fn write(value: f32, when: When) -> PendingWrite {
    PendingWrite {
        module_id: "osc".into(),
        key: "frequency".into(),
        value: value.into(),
        raw: None,
        beat: None,
        source: Source::Agent,
        when,
        expires: None,
        authored: Some(Authoring {
            value: ControlValue::from(value),
            origin: None,
            safe: None,
            then: Vec::new(),
            timed: when != When::Now,
            chained: false,
        }),
        announced: true,
        awaited: false,
    }
}

fn recorded(log: &mut PendingLog) -> usize {
    let steps = log.take_steps();
    (steps.iter())
        .filter(|step| matches!(step, DocumentStep::Authored { .. }))
        .count()
}

#[test]
fn a_timed_write_waiting_as_outcomes_are_lost_stays_for_its_own() {
    let (mut sender, receiver) = outcome_channel_reserving(4, 4);
    let mut log = PendingLog::new(receiver, Arc::default());
    let (timed, untimed) = (RequestId(1), RequestId(2));
    log.submitted(timed, write(330.0, When::AtSample(1_000_000)));
    log.submitted(untimed, write(440.0, When::Now));
    // More untimed outcomes than they have room for, none received: some
    // dropped, the untimed write's own maybe among them.
    for id in 10..20 {
        sender.send(RequestId(id), Outcome::Applied { at: 0 }, None);
    }

    let announced = log.take_announcements();
    let lost: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(lost, [(untimed, None)], "only the untimed write as applied");
    assert_eq!(recorded(&mut log), 1, "the untimed write recorded");
    assert!(log.waiting(timed), "the timed write still waits");
    assert!(!log.waiting(untimed));

    // More untimed outcomes, and the timed write's own, kept room for.
    for id in 20..30 {
        sender.send(RequestId(id), Outcome::Applied { at: 0 }, None);
    }
    sender.send_timed(timed, Outcome::Applied { at: 1_000_000 }, None);
    let [settled] = &Vec::from(log.take_announcements())[..] else {
        panic!("the timed write announced once it settles");
    };
    assert_eq!(settled.request, timed);
    assert_eq!(settled.outcome, Some(Outcome::Applied { at: 1_000_000 }));
    assert_eq!(recorded(&mut log), 1, "recorded as it applied");
    assert!(!log.waiting(timed));
}

#[test]
fn a_full_log_forgets_untimed_writes_before_a_timed_one() {
    let (_sender, receiver) = outcome_channel_reserving(4, 4);
    let mut log = PendingLog::new(receiver, Arc::default());
    let timed = RequestId(1);
    log.submitted(timed, write(330.0, When::AtSample(1_000_000)));
    // More untimed writes than the log remembers, none settled.
    for id in 2..2000 {
        log.submitted(RequestId(id), write(440.0, When::Now));
    }
    assert!(log.waiting(timed), "still waiting for its own outcome");
    assert!(
        !log.waiting(RequestId(2)),
        "the oldest untimed one forgotten"
    );
}

/// [`write`], timed on a sample, that may apply until `last`.
fn expiring(value: f32, at: u64, last: u64) -> PendingWrite {
    PendingWrite {
        expires: Some(last),
        ..write(value, When::AtSample(at))
    }
}

/// A timed write whose outcome never comes (lost past the room kept for
/// it, or held by a request the audio thread has not taken) settles
/// expired once the audio thread has rendered past its ttl, and is not
/// recorded; its outcome, should it come later, is not reported again.
#[test]
fn a_timed_write_past_its_ttl_with_no_outcome_settles_expired() {
    let (mut sender, receiver) = outcome_channel_reserving(4, 4);
    let transport = Arc::new(Transport::default());
    let mut log = PendingLog::new(receiver, Arc::clone(&transport));
    let id = RequestId(1);
    log.submitted(id, expiring(330.0, 900, 1_000));
    transport.publish(1_001 - 64);
    log.settle();
    assert!(log.waiting(id), "its ttl has not run out");
    transport.publish(1_001);
    let expired = Outcome::Refused(Refusal::Expired);
    let announced = log.take_announcements();
    let settled: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(settled, [(id, Some(expired))]);
    assert!(!log.waiting(id));
    assert_eq!(recorded(&mut log), 0, "never recorded");
    assert_eq!(log.take_outcomes(), (vec![(id, expired)], 0));

    sender.send_timed(id, expired, None);
    assert!(log.take_announcements().is_empty(), "not announced again");
    assert_eq!(log.take_outcomes(), (vec![], 0), "nor reported again");
}

/// One that applied by its last sample settles as it applied: its block
/// sent the outcome before the count passed its ttl.
#[test]
fn a_timed_write_applied_before_its_ttl_ran_out_settles_applied() {
    let (mut sender, receiver) = outcome_channel_reserving(4, 4);
    let transport = Arc::new(Transport::default());
    let mut log = PendingLog::new(receiver, Arc::clone(&transport));
    let id = RequestId(1);
    log.submitted(id, expiring(330.0, 900, 1_000));
    let applied = Outcome::Applied { at: 1_000 };
    sender.send_timed(id, applied, None);
    transport.publish(1_024);
    let announced = log.take_announcements();
    let settled: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(settled, [(id, Some(applied))]);
    assert_eq!(recorded(&mut log), 1);
}
