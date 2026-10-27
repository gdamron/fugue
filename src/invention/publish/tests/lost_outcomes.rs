//! Outcomes the audio thread drops when no control thread has made room:
//! an untimed write, or one timed on a sample already rendered, is taken
//! to have applied, as most do, but one timed on a sample yet to render
//! still waits, so it stays listed, unrecorded, until its own outcome
//! settles it.

use std::sync::Arc;

use crate::control_request::{outcome_channel, Outcome, RequestId, Source, Transport, When};
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
    for id in 10..14 {
        sender.send(RequestId(id), Outcome::Applied { at: 0 }, None);
    }
    sender.send(passed, Outcome::Applied { at: 100 }, None);

    let announced = log.take_announcements();
    let lost: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(
        lost,
        [(passed, None), (untimed, None)],
        "taken to have applied"
    );
    assert_eq!(recorded(&mut log), 2, "both recorded");
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
