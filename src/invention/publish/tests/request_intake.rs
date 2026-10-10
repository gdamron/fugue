//! Control request intake across publications and under pressure: a
//! request queued behind an edit, or in the pending store when its module
//! moves or goes, lands on the right instance or is refused; a full store
//! back-pressures producers, except ahead of a queued edit, which the
//! requests in front of it never starve. Every block here is allocation-
//! and free-free.

use super::requests::{counted_block, frequency, oscillator_rig, outcomes, submit, try_submit};
use super::*;
use crate::control_request::{Outcome, Refusal, When};
use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};

#[test]
fn a_request_queued_behind_a_deferred_edit_is_clean() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let id = submit(&rig, "osc3", port, 0.5, When::Now);

    // It waits in the queue behind the edit, which waits for room to retire
    // the publication it replaces.
    assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    assert_eq!(outcomes(&mut rig), []);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
    rig.live.reclaim();
    let start = rig.graph.current_sample;
    assert_eq!(
        counted_block(&mut rig),
        (0, 0),
        "applying after the install"
    );
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    assert_eq!(frequency(&mut rig, "osc3", port), 0.5);
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: start })]);
}

#[test]
fn remapping_a_waiting_request_across_an_install_is_clean() {
    let (mut rig, port) = oscillator_rig();
    let at = rig.graph.current_sample + 64 + 7;
    let moved = submit(&rig, "osc2", port, 0.5, When::AtSample(at));
    let gone = submit(&rig, "osc1", port, 0.25, When::AtSample(at));
    assert_eq!(counted_block(&mut rig), (0, 0), "waiting");

    // osc2 moves from index 1 to 0 while its request waits; osc1 goes.
    rig.live.remove_module("osc1").unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing and remapping");
    assert_eq!(rig.module_ids(), ["osc2", "dac"]);
    assert_eq!(frequency(&mut rig, "osc2", port), 0.5);
    assert_eq!(
        outcomes(&mut rig),
        [
            (gone, Outcome::Refused(Refusal::TargetGone)),
            (moved, Outcome::Applied { at }),
        ]
    );
}

/// Fills the pending store with far-future requests to `osc1`, a queue at
/// a time, then fills the queue once more, each at its own time.
fn fill_store_and_queue(rig: &mut Rig, port: u16) {
    let far = rig.graph.current_sample + 1_000_000;
    let mut next = 0;
    for round in 0..=PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
        for _ in 0..REQUEST_QUEUE_CAPACITY {
            submit(rig, "osc1", port, 0.5, When::AtSample(far + next));
            next += 1;
        }
        if round < PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
            assert_eq!(counted_block(rig), (0, 0), "filling");
        }
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
    assert_eq!(outcomes(rig), []);
}

/// Whether the request queue has no room left.
fn queue_full(rig: &Rig) -> bool {
    try_submit(rig, "osc2", 0, 0.0, When::Now).is_err()
}

#[test]
fn a_full_store_leaves_requests_queued() {
    let (mut rig, port) = oscillator_rig();
    fill_store_and_queue(&mut rig, port);
    let capacity = rig.graph.requests.as_ref().unwrap().pending.capacity();

    // Back-pressure: nothing is popped or refused, and producers see a full
    // queue rather than a refusal.
    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0), "holding back");
        assert_eq!(outcomes(&mut rig), []);
        assert!(queue_full(&rig));
    }
    let pending = &rig.graph.requests.as_ref().unwrap().pending;
    assert_eq!(pending.capacity(), capacity);
}

#[test]
fn an_edit_behind_a_full_store_drains_the_queue_ahead_of_it() {
    let (mut rig, port) = oscillator_rig();
    fill_store_and_queue(&mut rig, port);

    // The edit takes a slot the requests leave for edits, and the requests
    // ahead of it are popped and refused for want of room, rather than
    // keeping it from installing.
    rig.live
        .connect(edge("osc1", "audio", "osc2", "frequency_mod"))
        .unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    let settled = outcomes(&mut rig);
    assert_eq!(settled.len(), REQUEST_QUEUE_CAPACITY);
    assert!(settled
        .iter()
        .all(|(_, outcome)| *outcome == Outcome::Refused(Refusal::PendingFull)));
    assert!(!queue_full(&rig));
    let pending = &rig.graph.requests.as_ref().unwrap().pending;
    assert_eq!(pending.len(), PENDING_REQUEST_CAPACITY);
}

#[test]
fn a_full_store_still_takes_a_replacement_for_a_waiting_request() {
    let (mut rig, port) = oscillator_rig();
    let start = rig.graph.current_sample;
    // Two blocks fill the store; osc2's request is due 10 samples into the
    // block after them.
    let due = start + 2 * 64 + 10;
    let mut original = None;
    for n in 0..PENDING_REQUEST_CAPACITY as u64 {
        if n == 300 {
            original = Some(submit(&rig, "osc2", port, 0.25, When::AtSample(due)));
        } else {
            submit(
                &rig,
                "osc1",
                port,
                0.5,
                When::AtSample(start + 1_000_000 + n),
            );
        }
        if (n + 1) % REQUEST_QUEUE_CAPACITY as u64 == 0 {
            assert_eq!(counted_block(&mut rig), (0, 0), "filling");
        }
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());

    // The replacement needs no room, so it supersedes the waiting request
    // in time; a new request behind it still waits in the queue.
    let replacement = submit(&rig, "osc2", port, 0.75, When::AtSample(due));
    submit(&rig, "osc1", port, 0.5, When::AtSample(start + 2_000_000));
    assert_eq!(counted_block(&mut rig), (0, 0), "coalescing when full");
    assert_eq!(frequency(&mut rig, "osc2", port), 0.75);
    assert_eq!(
        outcomes(&mut rig),
        [
            (original.unwrap(), Outcome::Superseded),
            (replacement, Outcome::Applied { at: due }),
        ]
    );
}

/// Two edits and the requests submitted around them, all in one block:
/// each request acts on the graph it was submitted against, and nothing
/// allocates or frees.
#[test]
fn requests_and_edits_in_one_block_apply_in_order_cleanly() {
    let (mut rig, port) = oscillator_rig();
    let (generation, applied) = rig.generation_and_applied();
    let start = rig.graph.current_sample;
    let before = submit(&rig, "osc1", port, 0.5, When::Now);
    let timed = submit(&rig, "osc1", port, 0.75, When::AtSample(start + 64 + 9));
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let between = submit(&rig, "osc3", port, 0.25, When::Now);
    rig.publish_unreclaimed(|change| change.remove("osc1"));
    let after = submit(&rig, "osc3", port, 0.125, When::Now);

    assert_eq!(counted_block(&mut rig), (0, 0), "two installs in order");
    assert_eq!(rig.module_ids(), ["osc2", "dac", "osc3"]);
    assert_eq!(rig.generation_and_applied(), (generation + 2, applied + 2));
    assert_eq!(frequency(&mut rig, "osc3", port), 0.125);
    let applied = Outcome::Applied { at: start };
    assert_eq!(
        outcomes(&mut rig),
        [
            (before, applied),
            (between, applied),
            (timed, Outcome::Refused(Refusal::TargetGone)),
            (after, applied),
        ]
    );
}

/// Edits have the queue's reserve to themselves, but not without end:
/// once every slot is taken, an edit is refused whole, nothing published,
/// and the ones queued install in order once there is room.
#[test]
fn a_full_request_queue_refuses_an_edit_and_hands_the_change_back() {
    use crate::invention::publish::publisher::EDIT_RESERVE;
    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.hold_a_retirement();
    let (generation, applied) = rig.generation_and_applied();
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let mut queued = 0;
    let refused = loop {
        let mut publisher = rig.live.publisher().lock().unwrap();
        let mut change = rig.live.change_on(&publisher);
        if queued % 2 == 0 {
            change.disconnect(fm.clone());
        } else {
            change.connect(fm.clone()).unwrap();
        }
        match publisher.publish(change.prepare().unwrap()) {
            Ok(_) => queued += 1,
            Err(refused) => break refused,
        }
    };
    assert_eq!(queued, (REQUEST_QUEUE_CAPACITY + EDIT_RESERVE) as u64);
    assert!(matches!(refused.error, GraphCommandError::QueueFull));
    assert!(refused.change.publication.is_some(), "handed back whole");
    assert_eq!(rig.generation_and_applied(), (generation + queued, applied));
    // Control requests find no room either.
    assert!(try_submit(&rig, "osc1", 0, 0.5, When::Now).is_err());
    drop(refused);

    while rig.generation_and_applied().1 < applied + queued {
        rig.live.reclaim();
        assert_eq!(counted_block(&mut rig), (0, 0), "installing in order");
    }
    assert_eq!(
        rig.generation_and_applied(),
        (generation + queued, applied + queued)
    );
}

/// However fast the reclaimer makes room, a block installs a bounded
/// number of edits; the rest wait in the queue for the next.
#[test]
fn a_block_installs_at_most_its_share_of_queued_edits() {
    use crate::invention::graph::MAX_INSTALLS_PER_BLOCK;
    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.live.reclaim();
    let (generation, applied) = rig.generation_and_applied();
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let edits = MAX_INSTALLS_PER_BLOCK as u64 + 2;
    for n in 0..edits {
        rig.publish_unreclaimed(|change| {
            if n % 2 == 0 {
                change.connect(fm.clone()).unwrap();
            } else {
                change.disconnect(fm.clone());
            }
        });
    }
    assert_eq!(counted_block(&mut rig), (0, 0), "installing its share");
    let installed = MAX_INSTALLS_PER_BLOCK as u64;
    assert_eq!(
        rig.generation_and_applied(),
        (generation + edits, applied + installed)
    );
    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing the rest");
    assert_eq!(
        rig.generation_and_applied(),
        (generation + edits, applied + edits)
    );
}
