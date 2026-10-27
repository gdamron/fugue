//! Outcomes the audio thread drops when no control thread has made room:
//! an untimed write is taken to have applied, as most do, but a timed one
//! most likely still waits, so it stays listed, unrecorded, until its own
//! outcome settles it.

use crate::control_request::{outcome_channel, Outcome, RequestId, Source, When};
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
fn a_timed_write_waiting_as_outcomes_are_lost_stays_for_its_own() {
    let (mut sender, receiver) = outcome_channel(4);
    let mut log = PendingLog::new(receiver);
    let (timed, untimed) = (RequestId(1), RequestId(2));
    log.submitted(timed, write(330.0, When::AtSample(1_000_000)));
    log.submitted(untimed, write(440.0, When::Now));
    // More outcomes than the channel holds, none received: some dropped.
    for id in 10..20 {
        sender.send(RequestId(id), Outcome::Applied { at: 0 }, None);
    }

    let announced = log.take_announcements();
    let lost: Vec<_> = announced.iter().map(|a| (a.request, a.outcome)).collect();
    assert_eq!(lost, [(untimed, None)], "only the untimed write as applied");
    assert_eq!(recorded(&mut log), 1, "the untimed write recorded");
    assert!(log.waiting(timed), "the timed write still waits");
    assert!(!log.waiting(untimed));

    sender.send(timed, Outcome::Applied { at: 1_000_000 }, None);
    let [settled] = &Vec::from(log.take_announcements())[..] else {
        panic!("the timed write announced once it settles");
    };
    assert_eq!(settled.request, timed);
    assert_eq!(settled.outcome, Some(Outcome::Applied { at: 1_000_000 }));
    assert_eq!(recorded(&mut log), 1, "recorded as it applied");
    assert!(!log.waiting(timed));
}
