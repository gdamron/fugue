//! Requests that race a publication, the twins of `writes.rs`: requests and
//! edits apply in one order, the order they were submitted in, so each
//! request lands on the instance it was resolved against, waiting in the
//! queue behind an edit not yet installed, or is refused when an edit took
//! its target away while it waited for its sample.

use super::requests::{hook, outcomes, submit};
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

    // Blocks run on the old graph; the requests wait in the queue behind
    // the edit, not applied to whatever holds their indices there.
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
fn an_immediate_request_before_an_edit_acts_on_the_instances_it_replaces() {
    let (mut rig, probes) = probes(&["p1", "p2"]);
    let start = rig.graph.current_sample;
    let to_rebuilt = submit(&rig, "p1", 0, 1.0, When::Now);
    let to_removed = submit(&rig, "p2", 0, 2.0, When::Now);
    let rebuilt = probe(&rig, "p1");
    edit(&rig, |change| {
        change.upsert("p1", rebuilt);
        change.remove("p2");
    });
    rig.render(1);

    // Submitted before the edit, so applied before it installs.
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1"]);
    assert_eq!(
        probes.take(),
        [got("p1", 0, "a", 1.0), got("p2", 1, "a", 2.0)]
    );
    let applied = Outcome::Applied { at: start };
    assert_eq!(
        outcomes(&mut rig),
        [(to_rebuilt, applied), (to_removed, applied)]
    );
    submit(&rig, "p1", 0, 3.0, When::Now);
    rig.render(1);
    assert_eq!(probes.take(), [got("p1", 2, "a", 3.0)]);
}

/// The acceptance test for one serial order: control requests and edits
/// apply in the order they were submitted, however many edits are queued
/// before a block, and a waiting request whose module an edit rebuilds is
/// refused.
#[test]
fn control_requests_and_edits_apply_in_submit_order() {
    let (mut rig, probes) = probes(&["p1"]);
    let start = rig.graph.current_sample;
    // Generation 1: p1 (serial 0).
    let r1 = submit(&rig, "p1", 0, 1.0, When::Now);
    let timed = submit(&rig, "p1", 1, 9.0, When::AtSample(start + 64 + 7));
    // Generation 2 adds p2 (serial 1).
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    let r2 = submit(&rig, "p1", 0, 2.0, When::Now);
    let r3 = submit(&rig, "p2", 0, 3.0, When::Now);
    // Generation 3 rebuilds p1 (serial 2) and removes osc1.
    let rebuilt = probe(&rig, "p1");
    edit(&rig, |change| {
        change.upsert("p1", rebuilt);
        change.remove("osc1");
    });
    let r4 = submit(&rig, "p1", 0, 4.0, When::Now);
    rig.render(1);

    // Each request lands on the graph it was submitted against: r1 and r2
    // on the first p1 (never coalesced, an edit lies between them), r4 on
    // its replacement. The timed request waited for the first p1, which
    // the second edit rebuilt.
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_eq!(
        probes.take(),
        [
            got("p1", 0, "a", 1.0),
            got("p1", 0, "a", 2.0),
            got("p2", 1, "a", 3.0),
            got("p1", 2, "a", 4.0),
        ]
    );
    let applied = Outcome::Applied { at: start };
    assert_eq!(
        outcomes(&mut rig),
        [
            (r1, applied),
            (r2, applied),
            (r3, applied),
            (timed, Outcome::Refused(Refusal::TargetGone)),
            (r4, applied),
        ]
    );
    assert_eq!(rig.generation_and_applied(), (3, 3));
    rig.render(2);
    assert_eq!(probes.take(), []);
}

#[test]
fn a_request_between_two_edits_reaches_the_instance_the_first_built() {
    let (mut rig, probes) = probes(&["p1"]);
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    let id = submit(&rig, "p2", 0, 1.0, When::Now);
    let rebuilt = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", rebuilt));
    let start = rig.graph.current_sample;
    rig.render(1);

    assert_eq!(probes.take(), [got("p2", 1, "a", 1.0)]);
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: start })]);
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
fn a_request_behind_deferred_edits_lands_between_them() {
    let (mut rig, probes) = probes(&["p1"]);
    rig.hold_a_retirement();
    // Generation g + 1 adds p2 and waits to install; a request for it
    // waits in the queue behind it. Generation g + 2 moves p2.
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
