//! Outcomes the audio thread drops when no control thread has made room:
//! an untimed write is taken to have applied, as most do. One timed on a
//! sample stays listed, unrecorded, until its own outcome settles it or
//! until the drain has taken it up and the graph has rendered past it.

use std::sync::Arc;

use crate::control_request::{
    outcome_channel, request_channel, ControlIndex, ControlTarget, Outcome, OutcomeSender,
    QueueConsumer, Refusal, Request, RequestId, RequestSender, RequestValue, RtValue, Source,
    Transport, When,
};
use crate::invention::publish::{Authoring, DocumentStep, PendingLog, PendingWrite};
use crate::ControlValue;

/// A log over a channel of 4 outcomes and a request queue, with the
/// queue's far end and the transport the tests render on.
struct Rig {
    log: PendingLog,
    outcomes: OutcomeSender,
    requests: RequestSender,
    drain: QueueConsumer<Request>,
    transport: Arc<Transport>,
}

fn rig() -> Rig {
    let (outcomes, receiver) = outcome_channel(4);
    let transport = Arc::new(Transport::new());
    let (requests, drain) = request_channel(8, Arc::clone(&transport));
    let log = PendingLog::new(receiver, requests.clone());
    Rig {
        log,
        outcomes,
        requests,
        drain,
        transport,
    }
}

impl Rig {
    /// Queues a write of `osc.frequency` at `when` and lists it.
    fn queue(&mut self, value: f32, when: When) -> RequestId {
        let target = ControlTarget {
            generation: 0,
            module_idx: 0,
            control: ControlIndex(0),
        };
        let request = Request {
            when,
            ..Request::new(target, RequestValue::Value(RtValue::F32(value)))
        };
        let id = self.requests.submit(request).expect("room");
        self.log.submitted(id, write(value, when));
        id
    }

    /// The drain takes up every request queued.
    fn take_up(&mut self) {
        while self.drain.pop().is_some() {}
    }

    /// The drain takes up the next request queued.
    fn take_one(&mut self) {
        self.drain.pop().expect("queued");
    }

    /// Others' outcomes fill the channel, and `then` is dropped.
    fn drop_outcome(&mut self, then: RequestId, outcome: Outcome) {
        fill(&mut self.outcomes);
        self.outcomes.send(then, outcome, None);
    }

    fn announced(&mut self) -> Vec<(RequestId, Option<Outcome>)> {
        let announced = self.log.take_announcements();
        announced.iter().map(|a| (a.request, a.outcome)).collect()
    }
}

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
    let mut rig = rig();
    let ahead = rig.queue(330.0, When::AtSample(1_000_000));
    let passed = rig.queue(220.0, When::AtSample(100));
    let untimed = rig.queue(440.0, When::Now);
    rig.take_up();
    rig.transport.publish(4096);
    // The passed write's own outcome is dropped.
    rig.drop_outcome(passed, Outcome::Applied { at: 100 });

    assert_eq!(rig.announced(), [(untimed, None)], "taken to have applied");
    assert_eq!(recorded(&mut rig.log), 1);
    assert!(rig.log.waiting(passed), "seen taken up only from here");
    assert!(rig.announced().is_empty(), "not past the count read then");
    rig.transport.publish(4160);
    assert_eq!(rig.announced(), [(passed, None)], "taken to have applied");
    assert_eq!(recorded(&mut rig.log), 1);
    assert!(rig.log.waiting(ahead), "the write ahead still waits");
    assert_eq!(rig.log.on_sample(), 1);

    rig.outcomes
        .send(ahead, Outcome::Applied { at: 1_000_000 }, None);
    let settled = rig.announced();
    assert_eq!(settled, [(ahead, Some(Outcome::Applied { at: 1_000_000 }))]);
    assert_eq!(recorded(&mut rig.log), 1, "recorded as it applied");
    assert!(!rig.log.waiting(ahead));
}

#[test]
fn a_write_whose_outcome_was_lost_before_its_block_published_settles_once_rendered() {
    let mut rig = rig();
    let write_id = rig.queue(330.0, When::AtSample(5000));
    rig.take_up();
    rig.transport.publish(4992);
    // It applies in the block from 4992 and its outcome is dropped, seen
    // before that block's count is published.
    rig.drop_outcome(write_id, Outcome::Applied { at: 5000 });
    assert!(rig.announced().is_empty());
    assert!(rig.log.waiting(write_id), "its sample may still be ahead");

    // No more outcomes drop: rendering on past it settles it anyway.
    rig.transport.publish(5056);
    assert!(rig.announced().is_empty(), "seen taken up at 5056");
    rig.transport.publish(5120);
    assert_eq!(rig.announced(), [(write_id, None)], "taken to have applied");
    assert_eq!(recorded(&mut rig.log), 1);
    assert!(!rig.log.waiting(write_id));
    assert_eq!(rig.log.on_sample(), 0, "its place under the bound freed");
}

#[test]
fn a_suspect_write_still_queued_waits_however_far_rendering_goes() {
    let mut rig = rig();
    let write_id = rig.queue(330.0, When::AtSample(5000));
    // Left queued (behind an edit that cannot install, say) while another
    // request's outcome is dropped.
    rig.drop_outcome(RequestId(20), Outcome::Applied { at: 0 });
    for rendered in [10_000, 100_000, 1_000_000] {
        rig.transport.publish(rendered);
        assert!(rig.announced().is_empty());
    }
    assert!(rig.log.waiting(write_id), "never taken to have applied");

    // Taken up at last, past its ttl.
    rig.take_up();
    rig.outcomes.send(write_id, Refusal::Expired.into(), None);
    rig.transport.publish(1_000_064);
    assert_eq!(rig.announced(), [(write_id, Some(Refusal::Expired.into()))]);
    assert_eq!(recorded(&mut rig.log), 0, "refused: nothing authored");
    assert!(!rig.log.waiting(write_id));
}

#[test]
fn a_suspect_write_refused_later_settles_as_refused() {
    let mut rig = rig();
    let write_id = rig.queue(330.0, When::AtSample(100_000));
    rig.take_up();
    rig.drop_outcome(RequestId(20), Outcome::Applied { at: 0 });
    assert!(rig.announced().is_empty());
    rig.transport.publish(64);
    assert!(rig.announced().is_empty(), "seen taken up");

    // Its refusal is queued in the block that also renders past it: it is
    // received before anything is taken to have applied.
    rig.outcomes
        .send(write_id, Refusal::TargetGone.into(), None);
    rig.transport.publish(200_000);
    assert_eq!(
        rig.announced(),
        [(write_id, Some(Refusal::TargetGone.into()))]
    );
    assert_eq!(recorded(&mut rig.log), 0, "refused: nothing authored");
    assert!(!rig.log.waiting(write_id));
}

#[test]
fn suspects_queued_apart_each_settle_past_where_they_were_taken_up() {
    let mut rig = rig();
    let first = rig.queue(330.0, When::AtSample(100));
    rig.drop_outcome(RequestId(20), Outcome::Applied { at: 0 });
    assert!(rig.announced().is_empty());
    let second = rig.queue(220.0, When::AtSample(200));
    rig.drop_outcome(RequestId(21), Outcome::Applied { at: 0 });
    assert!(rig.announced().is_empty());

    rig.take_one();
    rig.transport.publish(4096);
    assert!(
        rig.announced().is_empty(),
        "the first seen taken up at 4096"
    );
    rig.transport.publish(4160);
    assert_eq!(rig.announced(), [(first, None)], "taken to have applied");
    assert!(rig.log.waiting(second), "the second still queued");

    rig.take_up();
    assert!(
        rig.announced().is_empty(),
        "the second seen taken up at 4160"
    );
    rig.transport.publish(4224);
    assert_eq!(rig.announced(), [(second, None)], "taken to have applied");
    assert_eq!(recorded(&mut rig.log), 2);
    assert_eq!(rig.log.on_sample(), 0);
}
