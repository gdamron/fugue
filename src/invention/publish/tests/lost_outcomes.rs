//! Outcomes the audio thread drops when no control thread has made room:
//! an untimed write is taken to have applied, as most do. One timed on a
//! sample stays listed, unrecorded, until its own outcome settles it or
//! until the graph has surely rendered past it.

use std::sync::Arc;

use crate::control_request::{
    outcome_channel, Outcome, OutcomeSender, Refusal, RequestId, Source, Transport, When,
};
use crate::invention::publish::{Authoring, DocumentStep, PendingLog, PendingWrite};
use crate::{ControlValue, MAX_BLOCK};

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

/// Fills a channel of 4 with others' outcomes and one more, so the next
/// outcome sent is dropped.
fn fill(sender: &mut OutcomeSender) {
    for id in 10..14 {
        sender.send(RequestId(id), Outcome::Applied { at: 0 }, None);
    }
}

fn recorded(log: &mut PendingLog) -> usize {
    let steps = log.take_steps();
    (steps.iter())
        .filter(|step| matches!(step, DocumentStep::Authored { .. }))
        .count()
}

#[test]
fn a_write_timed_on_a_sample_yet_to_render_stays_through_lost_outcomes() {
    let (mut sender, receiver) = outcome_channel(4);
    let transport = Arc::new(Transport::new());
    let mut log = PendingLog::new(receiver, Arc::clone(&transport));
    let (ahead, passed, untimed) = (RequestId(1), RequestId(2), RequestId(3));
    log.submitted(ahead, write(330.0, When::AtSample(1_000_000)));
    log.submitted(passed, write(220.0, When::AtSample(100)));
    log.submitted(untimed, write(440.0, When::Now));
    transport.publish(4096);
    // More outcomes than the channel holds, none received: the newest
    // dropped, the passed write's own among them.
    fill(&mut sender);
    sender.send(passed, Outcome::Applied { at: 100 }, None);

    let announced = log.take_announcements();
    let lost: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(lost, [(untimed, None)], "taken to have applied");
    assert_eq!(recorded(&mut log), 1);
    assert!(log.waiting(passed), "maybe taken up only in the next block");
    // Once a block past where it was surely taken up has rendered.
    transport.publish(4096 + MAX_BLOCK as u64 + 1);
    let announced = log.take_announcements();
    let lost: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(lost, [(passed, None)], "taken to have applied");
    assert_eq!(recorded(&mut log), 1);
    assert!(log.waiting(ahead), "the write ahead still waits");
    assert_eq!(log.on_sample(), 1);

    sender.send(ahead, Outcome::Applied { at: 1_000_000 }, None);
    let [settled] = &Vec::from(log.take_announcements())[..] else {
        panic!("the write ahead announced once it settles");
    };
    assert_eq!(settled.request, ahead);
    assert_eq!(settled.outcome, Some(Outcome::Applied { at: 1_000_000 }));
    assert_eq!(recorded(&mut log), 1, "recorded as it applied");
    assert!(!log.waiting(ahead));
}

#[test]
fn a_write_whose_outcome_was_lost_before_its_block_published_settles_once_rendered() {
    let (mut sender, receiver) = outcome_channel(4);
    let transport = Arc::new(Transport::new());
    let mut log = PendingLog::new(receiver, Arc::clone(&transport));
    let write_id = RequestId(1);
    log.submitted(write_id, write(330.0, When::AtSample(5000)));
    transport.publish(4992);
    // It applies in the block from 4992 and its outcome is dropped, seen
    // before that block's count is published.
    fill(&mut sender);
    sender.send(write_id, Outcome::Applied { at: 5000 }, None);
    assert!(log.take_announcements().is_empty());
    assert!(log.waiting(write_id), "its sample may still be ahead");

    // No more outcomes drop: rendering on past it settles it anyway.
    transport.publish(4992 + MAX_BLOCK as u64 + 1);
    let [settled] = &Vec::from(log.take_announcements())[..] else {
        panic!("settled once rendered past");
    };
    assert_eq!((settled.request, settled.outcome), (write_id, None));
    assert_eq!(recorded(&mut log), 1, "taken to have applied");
    assert!(!log.waiting(write_id));
    assert_eq!(log.on_sample(), 0, "its place under the bound freed");
}

#[test]
fn a_suspect_write_refused_later_settles_as_refused() {
    let (mut sender, receiver) = outcome_channel(4);
    let transport = Arc::new(Transport::new());
    let mut log = PendingLog::new(receiver, Arc::clone(&transport));
    let write_id = RequestId(1);
    log.submitted(write_id, write(330.0, When::AtSample(100_000)));
    fill(&mut sender);
    sender.send(RequestId(20), Outcome::Applied { at: 0 }, None);
    assert!(log.take_announcements().is_empty());
    log.take_outcomes();

    // Its refusal is queued in the block that also renders past it: it is
    // received before anything is taken to have applied.
    sender.send(write_id, Refusal::TargetGone.into(), None);
    transport.publish(200_000);
    let [settled] = &Vec::from(log.take_announcements())[..] else {
        panic!("settled once");
    };
    assert_eq!(settled.request, write_id);
    assert_eq!(settled.outcome, Some(Refusal::TargetGone.into()));
    assert_eq!(recorded(&mut log), 0, "refused: nothing authored");
    assert!(!log.waiting(write_id));
}
