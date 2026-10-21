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
