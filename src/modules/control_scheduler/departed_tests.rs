//! A scheduler that leaves the running graph, or a schedule replaced,
//! holds no other schedule's writes back.

use super::legacy_tests::{block, entry, queued_notes, scheduler, Locking};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::while_held;

#[test]
fn a_removed_scheduler_s_waiting_note_holds_no_other_scheduler_s_back() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let note = |key| vec![entry(1, "sample_instrument", "note_on", Number(key))];
    let mut removed = scheduler(note(64.0), &directory);
    let mut staying = scheduler(note(67.0), &directory);
    for gate in [1.0, 0.0] {
        block(&mut removed, gate);
        block(&mut staying, gate);
    }
    // The removed one's note finds the queue held, and is still waiting
    // as an edit takes the scheduler out of the graph.
    let ((), allocs, frees) = while_held(locking.instrument.pending_notes(), || {
        block(&mut removed, 1.0);
        allocator_events(|| removed.leave())
    });
    assert_eq!((allocs, frees), (0, 0), "left");
    // Never processed again, it is not freed yet either: the other's note
    // still lands at once.
    assert_eq!(block(&mut staying, 1.0), (0, 0));
    assert_eq!(queued_notes(&locking.instrument), [(67, true)]);
}

#[test]
fn a_removed_scheduler_s_waiting_note_lands_as_it_leaves_if_it_can() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let notes = vec![
        entry(1, "sample_instrument", "note_on", Number(64.0)),
        entry(2, "sample_instrument", "note_off", Number(64.0)),
    ];
    let mut removed = scheduler(notes, &directory);
    for gate in [1.0, 0.0, 1.0, 0.0] {
        block(&mut removed, gate);
    }
    // Step 2's note_off finds the queue held; it is free again by the
    // time the edit removing the scheduler installs. Made before the edit,
    // it lands, as main's waiting write would have: no stuck note.
    while_held(locking.instrument.pending_notes(), || {
        block(&mut removed, 1.0)
    });
    removed.leave();
    assert_eq!(queued_notes(&locking.instrument), [(64, true), (64, false)]);
}
