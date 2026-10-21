//! A scheduler that leaves the running graph, or a schedule replaced,
//! holds no other schedule's writes back.

use super::legacy_tests::{block, entry, queued_notes, scheduler, Locking};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{land_left_events, while_held};

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
    assert_eq!(removed.controls().refused_writes(), 0, "left, not dropped");
    // Never processed again, it is not freed yet either: the other's note
    // still lands at once, just after the removed one's, made before it
    // and left at the gate, as main's waiting write would have landed.
    assert_eq!(block(&mut staying, 1.0), (0, 0));
    assert_eq!(queued_notes(&locking.instrument), [(64, true), (67, true)]);
}

#[test]
fn a_ramp_in_a_new_schedule_starts_where_the_old_one_s_waiting_write_leaves_it() {
    use super::resolved::ResolvedSchedule;
    use super::schedule::resolve_schedule;
    use crate::payload::Payload;
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let degree = |at, to| entry(at, "melody", "degree.0", Number(to));
    let mut scheduler = scheduler(vec![degree(1, 12.0)], &directory);
    for gate in [1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    // The new schedule ramps the same degree up from step 2.
    let surfaces = directory.lock().unwrap().clone();
    let rising = vec![ScheduleEntry {
        ramp_steps: Some(4),
        ..degree(2, 24.0)
    }];
    let next = ResolvedSchedule::new(resolve_schedule(&rising, "sched", &surfaces).unwrap());
    let next = Payload::owned(Box::new(next));
    // Step 1's 12 finds the table held, and still waits in the schedule
    // replaced as step 2's ramp starts.
    let (old, started) = while_held(&locking.melody.table, || {
        block(&mut scheduler, 1.0);
        let old = scheduler.apply_payload(SCHEDULE, next);
        block(&mut scheduler, 0.0);
        (old, block(&mut scheduler, 1.0))
    });
    assert_eq!(started, (0, 0), "step 2");
    drop(old);
    block(&mut scheduler, 0.0);
    // It rises from 12, not from the 0 the table held.
    let rising = locking.melody.degree(0).unwrap();
    assert!((12..=15).contains(&rising), "{rising}");
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

#[test]
fn a_note_off_a_removed_scheduler_made_behind_another_s_note_on_still_lands() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let note = |control| vec![entry(1, "sample_instrument", control, Number(64.0))];
    let mut playing = scheduler(note("note_on"), &directory);
    let mut removed = scheduler(note("note_off"), &directory);
    for gate in [1.0, 0.0] {
        block(&mut playing, gate);
        block(&mut removed, gate);
    }
    // The note_on finds the queue held; the note_off, made after it by the
    // other scheduler, waits behind it.
    while_held(locking.instrument.pending_notes(), || {
        block(&mut playing, 1.0);
        block(&mut removed, 1.0);
    });
    // The edit removing the second installs before the first retries.
    let ((), allocs, frees) = allocator_events(|| removed.leave());
    assert_eq!((allocs, frees), (0, 0), "left");
    assert_eq!(queued_notes(&locking.instrument), [], "behind the note_on");
    assert_eq!(block(&mut playing, 0.0), (0, 0));
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false)],
        "no stuck note"
    );
}

#[test]
fn a_removed_scheduler_counts_the_values_it_drops() {
    let locking = Locking::new();
    let directory = locking.directory();
    let degree = vec![entry(1, "melody", "degree.0", ScheduleValue::Number(5.0))];
    let mut removed = scheduler(degree, &directory);
    for gate in [1.0, 0.0] {
        block(&mut removed, gate);
    }
    while_held(&locking.melody.table, || {
        block(&mut removed, 1.0);
        removed.leave();
    });
    assert_eq!(removed.controls().refused_writes(), 1);
}

#[test]
fn a_note_off_the_only_scheduler_left_waiting_lands_once_it_is_freed() {
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
    // Step 2's note_off finds the queue held, and it still is as the edit
    // removing the scheduler installs, and as the reclaimer frees it: no
    // other schedule writes there.
    let queue = locking.instrument.pending_notes();
    let ((), allocs, frees) = while_held(queue, || {
        block(&mut removed, 1.0);
        let left = allocator_events(|| removed.leave());
        drop(removed);
        land_left_events();
        left
    });
    assert_eq!((allocs, frees), (0, 0), "left");
    assert_eq!(queued_notes(&locking.instrument), [(64, true)], "held");
    // The reclaimer's next pass lands it: no stuck note.
    land_left_events();
    assert_eq!(queued_notes(&locking.instrument), [(64, true), (64, false)]);
}

#[test]
fn a_new_scheduler_s_note_lands_after_the_note_off_a_freed_one_left() {
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
    // Step 2's note_off is left as the scheduler goes, and still held as
    // it is freed. Before the reclaimer's next pass, an edit adds another
    // that plays the note again: its gate is the one holding the note_off.
    let again = vec![entry(1, "sample_instrument", "note_on", Number(64.0))];
    let mut again = while_held(locking.instrument.pending_notes(), || {
        block(&mut removed, 1.0);
        removed.leave();
        drop(removed);
        let mut again = scheduler(again, &directory);
        for gate in [1.0, 0.0, 1.0] {
            block(&mut again, gate);
        }
        again
    });
    for gate in [0.0, 0.0] {
        block(&mut again, gate);
    }
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false), (64, true)],
        "the note off first"
    );
}
