//! Requests that race a publication, the twins of `writes.rs`: each lands
//! on the instance it was resolved against, waits for a publication not
//! yet installed, or is refused when its target went away, whether it is
//! still in the queue or waiting for its sample.

use super::requests::{hook, outcomes, submit, try_submit};
use super::writes::{edit, got, probe, rig_with_probes, WriteProbeFactory};
use super::*;
use crate::control_request::{Outcome, Refusal, When};

/// Control 0 is a probe's input `a`, control 1 its input `b`.
fn probes(ids: &[&str]) -> (Rig, WriteProbeFactory) {
    let (mut rig, probes) = rig_with_probes(ids);
    hook(&mut rig);
    (rig, probes)
}

#[test]
fn a_request_follows_its_module_to_a_new_index() {
    let (mut rig, probes) = probes(&["p1", "p2"]);
    // Resolved with p1 at index 3; removing osc1 moves p1 to 2 and puts p2
    // at 3 before the audio thread runs.
    submit(&rig, "p1", 1, 1.0, When::Now);
    rig.live.remove_module("osc1").unwrap();
    rig.render(1);

    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_eq!(probes.take(), [got("p1", 0, "b", 1.0)]);
}

#[test]
fn a_request_for_a_pending_publication_waits_for_its_install() {
    let (mut rig, probes) = probes(&["p1", "p2"]);
    rig.hold_a_retirement();
    let p3 = probe(&rig, "p3");
    rig.publish_unreclaimed(|change| {
        change.remove("osc1");
        change.upsert("p3", p3);
    });
    submit(&rig, "p2", 0, 2.0, When::Now);
    submit(&rig, "p3", 1, 3.0, When::Now);

    // Blocks run on the old graph; the requests are held, not applied to
    // whatever holds their indices there.
    rig.render(2);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1", "p2"]);
    assert_eq!(probes.take(), []);
    assert_eq!(outcomes(&mut rig), []);

    rig.live.reclaim();
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2", "p3"]);
    assert_eq!(
        probes.take(),
        [got("p2", 1, "a", 2.0), got("p3", 2, "b", 3.0)]
    );
}

#[test]
fn a_request_to_a_removed_or_rebuilt_module_is_refused() {
    let (mut rig, probes) = probes(&["p1", "p2"]);
    let rebuilt_target = submit(&rig, "p1", 0, 1.0, When::Now);
    let removed_target = submit(&rig, "p2", 0, 2.0, When::Now);
    let rebuilt = probe(&rig, "p1");
    edit(&rig, |change| {
        change.upsert("p1", rebuilt);
        change.remove("p2");
    });
    rig.render(1);

    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1"]);
    assert_eq!(probes.take(), []);
    let gone = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(
        outcomes(&mut rig),
        [(rebuilt_target, gone), (removed_target, gone)]
    );
    submit(&rig, "p1", 0, 3.0, When::Now);
    rig.render(1);
    assert_eq!(probes.take(), [got("p1", 2, "a", 3.0)]);
}

#[test]
fn a_request_for_a_folded_publication_reaches_its_instance() {
    let (mut rig, probes) = probes(&["p1"]);
    // Against the running graph (generation 1).
    submit(&rig, "p1", 0, 1.0, When::Now);
    // Generation 2 adds p2; these resolve against it.
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    submit(&rig, "p2", 0, 2.0, When::Now);
    submit(&rig, "p1", 0, 3.0, When::Now);
    // Generation 3 removes osc1 and absorbs the untaken generation 2.
    edit(&rig, |change| change.remove("osc1"));
    submit(&rig, "p2", 0, 4.0, When::Now);
    rig.render(1);

    // Every request follows its module into the installed order; requests
    // for the same control at the same sample coalesce, last wins.
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_eq!(
        probes.take(),
        [got("p1", 0, "a", 3.0), got("p2", 1, "a", 4.0)]
    );
    let superseded = outcomes(&mut rig)
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Superseded)
        .count();
    assert_eq!(superseded, 2);
}

#[test]
fn a_folded_request_to_a_module_rebuilt_by_the_fold_is_refused() {
    let (mut rig, probes) = probes(&["p1"]);
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    let id = submit(&rig, "p2", 0, 1.0, When::Now);
    let rebuilt = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", rebuilt));
    rig.render(1);

    assert_eq!(probes.take(), []);
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::TargetGone))]
    );
}

#[test]
fn waiting_requests_follow_their_modules_across_an_install() {
    let (mut rig, probes) = probes(&["p1", "p2"]);
    let at = rig.graph.current_sample + 2 * 64 + 5;
    submit(&rig, "p2", 1, 1.0, When::AtSample(at));
    let gone = submit(&rig, "p1", 1, 2.0, When::AtSample(at));
    rig.render(1);

    // While they wait, osc1 and p1 go: p2 moves from index 4 to 2.
    edit(&rig, |change| {
        change.remove("osc1");
        change.remove("p1");
    });
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p2"]);
    assert_eq!(probes.take(), []);
    assert_eq!(
        outcomes(&mut rig),
        [(gone, Outcome::Refused(Refusal::TargetGone))]
    );

    rig.render(1);
    assert_eq!(probes.take(), [got("p2", 1, "b", 1.0)]);
}

#[test]
fn a_held_request_waits_and_then_lands_through_a_fold() {
    let (mut rig, probes) = probes(&["p1"]);
    rig.hold_a_retirement();
    // Generation g + 1 adds p2 and stays untaken; a request for it is
    // held. Generation g + 2 folds it in and moves p2.
    let p2 = probe(&rig, "p2");
    rig.publish_unreclaimed(|change| change.upsert("p2", p2));
    submit(&rig, "p2", 1, 1.0, When::Now);
    rig.render(1);
    assert_eq!(probes.take(), []);
    rig.publish_unreclaimed(|change| change.remove("osc1"));
    rig.render(1);
    assert_eq!(probes.take(), []);

    rig.live.reclaim();
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_eq!(probes.take(), [got("p2", 1, "b", 1.0)]);
}

#[test]
fn deferred_installs_bound_the_folded_remaps_by_back_pressure() {
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let (mut rig, probes) = probes(&["p1"]);
    rig.hold_a_retirement();
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let far = rig.graph.current_sample + 1_000_000;
    let (per_round, rounds) = (100, 12);
    let mut next = 0;
    let mut written_rounds = 0;
    let mut queue_full = 0;
    // Each round publishes a generation that folds into the untaken one,
    // then submits against it, beyond what the store and queue can hold.
    for round in 0..rounds {
        rig.publish_unreclaimed(|change| {
            if round % 2 == 0 {
                change.connect(fm.clone()).unwrap();
            } else {
                change.disconnect(fm.clone());
            }
        });
        let mut written = false;
        for _ in 0..per_round {
            match try_submit(&rig, "p1", 0, 1.0, When::AtSample(far + next)) {
                Ok(_) => written = true,
                Err(_) => queue_full += 1,
            }
            next += 1;
        }
        written_rounds += usize::from(written);
        rig.render(1);
    }
    let capacity = PENDING_REQUEST_CAPACITY + REQUEST_QUEUE_CAPACITY;
    assert_eq!(queue_full, rounds * per_round - capacity);
    assert_eq!(outcomes(&mut rig), [], "nothing was refused");
    // Only generations with requests outstanding keep a remap: the later
    // rounds were all refused at the queue and keep none.
    let absorbed = {
        let publisher = rig.live.publisher().lock().unwrap();
        publisher.pending_absorbed().unwrap()
    };
    assert_eq!(absorbed.len(), written_rounds);
    assert_eq!(written_rounds, capacity.div_ceil(per_round));
    assert!(written_rounds < rounds);

    // The install maps every held request onto p1; the store's worth stays
    // pending and the queue's worth is refused for want of room.
    rig.live.reclaim();
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1"]);
    let settled = outcomes(&mut rig);
    assert_eq!(settled.len(), REQUEST_QUEUE_CAPACITY);
    assert!(settled
        .iter()
        .all(|(_, outcome)| *outcome == Outcome::Refused(Refusal::PendingFull)));
    assert_eq!(probes.take(), []);
}
