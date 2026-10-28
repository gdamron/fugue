//! A request waiting on another applying replacing one waiting for the
//! same control on the same request, by rank, and nothing else replacing.

use super::super::tests::{insert, log, play_ordered, request, store, table, Host};
use crate::alloc_counter::allocator_events;
use crate::control_request::condition::{Condition, Predicate};
use crate::control_request::hold::{Holds, Standing};
use crate::control_request::pending::{Outcome, Refusal};
use crate::control_request::request::{Request, When};
use crate::control_request::{ControlIndex, RequestId};

const AFTER: [Predicate; 1] = [Predicate::Applied(RequestId(7))];

#[test]
fn the_later_of_equal_rank_replaces_the_waiting_one_in_its_slot() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    for id in 1..=3 {
        insert(&mut conditions, &slots, request(id, &AFTER), &mut store);
    }
    let superseded = Outcome::Superseded;
    assert_eq!(log(&outcomes), [(1, superseded), (2, superseded)]);
    assert_eq!((conditions.len(), slots.taken()), (1, 1));
    assert_eq!(conditions.entries[0].request.id, RequestId(3));
}

#[test]
fn a_lower_rank_loses_to_the_one_waiting() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    insert(&mut conditions, &slots, request(1, &AFTER), &mut store);
    let mut lower = request(2, &AFTER);
    lower.priority = -1;
    insert(&mut conditions, &slots, lower, &mut store);
    assert_eq!(log(&outcomes), [(2, Outcome::Superseded)]);
    assert_eq!(conditions.entries[0].request.id, RequestId(1));
    assert_eq!(slots.taken(), 1);
}

#[test]
fn an_event_a_ttl_or_another_condition_replaces_nothing() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(8);
    insert(&mut conditions, &slots, request(1, &AFTER), &mut store);
    let mut event = request(2, &AFTER);
    event.event = true;
    let mut ttl = request(3, &AFTER);
    ttl.expires = Some(1_000);
    let other = request(4, &[Predicate::Applied(RequestId(8))]);
    let edges = [Predicate::edges(0, 0, 1)];
    for request in [event, ttl, other, request(5, &edges), request(6, &edges)] {
        insert(&mut conditions, &slots, request, &mut store);
    }
    assert_eq!(log(&outcomes), []);
    assert_eq!((conditions.len(), slots.taken()), (6, 6));
}

/// A waiting request that may expire is never replaced: a higher rank
/// expiring before the landing would otherwise leave nothing to apply.
#[test]
fn a_request_that_may_expire_is_never_replaced() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut expiring = request(1, &AFTER);
    expiring.expires = Some(100);
    expiring.priority = 1;
    insert(&mut conditions, &slots, expiring, &mut store);
    insert(&mut conditions, &slots, request(2, &AFTER), &mut store);
    assert_eq!(log(&outcomes), []);
    assert_eq!((conditions.len(), slots.taken()), (2, 2));
}

/// Request `id`, a copy of `write` kept over edit `edit`.
fn copy(id: u64, edit: u64, write: u64) -> Request {
    let mut copy = request(id, &AFTER);
    copy.when = When::Condition(Condition::kept(RequestId(edit), RequestId(write)));
    copy
}

/// Each copy waits on its own write, the latest the author's: it replaces
/// the copy waiting, of a higher rank too, allocating and freeing nothing.
#[test]
fn a_later_kept_copy_replaces_the_one_waiting_whatever_their_ranks() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut first = copy(1, 7, 10);
    first.priority = 1;
    insert(&mut conditions, &slots, first, &mut store);
    let second = copy(2, 7, 11);
    let ((), allocs, frees) =
        allocator_events(|| insert(&mut conditions, &slots, second, &mut store));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(log(&outcomes), [(1, Outcome::Superseded)]);
    assert_eq!((conditions.len(), slots.taken()), (1, 1));
    assert_eq!(conditions.entries[0].request.id, RequestId(2));
}

/// A copy replaces only a copy over the same edit: never the edit's own
/// write waiting on the landing, which plays should the copy break.
#[test]
fn a_kept_copy_replaces_only_a_copy_over_the_same_edit() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(8);
    for request in [request(1, &AFTER), copy(2, 8, 10), copy(3, 7, 11)] {
        insert(&mut conditions, &slots, request, &mut store);
    }
    insert(&mut conditions, &slots, request(4, &AFTER), &mut store);
    assert_eq!(log(&outcomes), [(1, Outcome::Superseded)]);
    insert(&mut conditions, &slots, copy(5, 7, 12), &mut store);
    assert_eq!(log(&outcomes), [(3, Outcome::Superseded)]);
    let ids: Vec<_> = conditions.entries.iter().map(|e| e.request.id.0).collect();
    assert_eq!(ids, [2, 4, 5]);
}

/// A copy whose write lost at its own sample is refused once the table
/// learns it, before the edit lands, so the value that never played is
/// not re-asserted there.
#[test]
fn a_kept_copy_whose_write_lost_is_refused() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, copy(1, 7, 10), &mut store);
    let lost = request(10, &AFTER);
    store.outcomes.settle(lost, Outcome::Superseded);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    let landed = Outcome::Applied { at: 70 };
    store.outcomes.settle(request(7, &AFTER), landed);
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert_eq!(
        log(&outcomes),
        [(10, Outcome::Superseded), (1, broken), (7, landed)]
    );
    assert!(host.applied.is_empty(), "{:?}", host.applied);
    assert_eq!((conditions.len(), slots.taken()), (0, 0));
}

/// Its write applied, the copy applies as the edit lands.
#[test]
fn a_kept_copy_applies_as_the_edit_lands_once_its_write_applied() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, copy(1, 7, 10), &mut store);
    store
        .outcomes
        .settle(request(10, &AFTER), Outcome::Applied { at: 0 });
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    assert!(host.applied.is_empty());
    store
        .outcomes
        .settle(request(7, &AFTER), Outcome::Applied { at: 70 });
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    let applied = log(&outcomes).last().copied();
    assert_eq!(applied, Some((1, Outcome::Applied { at: 70 })));
    assert_eq!(host.applied, [(9, 70)]);
}

/// Plays edit 7 landing at 70 with a copy kept over it (write 10 applied
/// first) and `write`, waiting on the edit alone, beside it: received
/// first (the edit's own), or after the copy when `copy_first` (one
/// scheduled after the edit). What settled, and how many requests reached
/// the module.
fn land_beside(write: Request, copy_first: bool) -> (Vec<(u64, Outcome)>, usize) {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let mut queue = [write, copy(2, 7, 10)];
    if copy_first {
        queue.reverse();
    }
    for request in queue {
        insert(&mut conditions, &slots, request, &mut store);
    }
    store
        .outcomes
        .settle(request(10, &AFTER), Outcome::Applied { at: 0 });
    store
        .outcomes
        .settle(request(7, &AFTER), Outcome::Applied { at: 70 });
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    assert_eq!((conditions.len(), slots.taken()), (0, 0));
    let settled = log(&outcomes)
        .into_iter()
        .filter(|(id, _)| [1, 2].contains(id));
    (settled.collect(), host.applied.len())
}

const LANDED: Outcome = Outcome::Applied { at: 70 };

/// As the edit lands, its own write gives way to the copy received after
/// it before either reaches the module, so one setter runs (a reverb
/// retunes once).
#[test]
fn an_edits_write_gives_way_to_the_copy_landing_with_it() {
    let (settled, applied) = land_beside(request(1, &AFTER), false);
    assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
    assert_eq!(applied, 1);
}

/// A write scheduled after the edit, received after the copy, is the later
/// write: it plays, and the copy gives way.
#[test]
fn a_write_scheduled_after_the_copy_plays_in_its_place() {
    let (settled, applied) = land_beside(request(1, &AFTER), true);
    assert_eq!(settled, [(2, Outcome::Superseded), (1, LANDED)]);
    assert_eq!(applied, 1);
}

/// Rank decides before receipt order: a write outranking the copy plays
/// whenever it came, and one the copy outranks gives way.
#[test]
fn the_higher_rank_plays_as_the_edit_lands_whichever_came_first() {
    for copy_first in [false, true] {
        let mut write = request(1, &AFTER);
        write.priority = 1;
        let (settled, applied) = land_beside(write, copy_first);
        assert_eq!(settled, [(2, Outcome::Superseded), (1, LANDED)]);
        assert_eq!(applied, 1);
        let mut write = request(1, &AFTER);
        write.priority = -1;
        let (settled, applied) = land_beside(write, copy_first);
        assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
        assert_eq!(applied, 1);
    }
}

/// A write with a ttl not yet run out replaces nothing on arrival, but
/// gives way as it lands with the copy received after it: one setter.
#[test]
fn a_write_with_a_ttl_gives_way_to_the_copy_landing_with_it() {
    let mut write = request(1, &AFTER);
    write.expires = Some(1_000);
    let (settled, applied) = land_beside(write, false);
    assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
    assert_eq!(applied, 1);
}

/// Writes to one control on different conditions met at one sample: the
/// later received plays, alone.
#[test]
fn writes_on_different_conditions_met_together_apply_once() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let on_8 = [Predicate::Applied(RequestId(8))];
    insert(&mut conditions, &slots, request(1, &AFTER), &mut store);
    insert(&mut conditions, &slots, request(2, &on_8), &mut store);
    for id in [7, 8] {
        let settled = request(id, &AFTER);
        store.outcomes.settle(settled, Outcome::Applied { at: 70 });
    }
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    assert_eq!((conditions.len(), slots.taken()), (0, 0));
    let settled: Vec<_> = (log(&outcomes).into_iter())
        .filter(|(id, _)| [1, 2].contains(id))
        .collect();
    assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
    assert_eq!(host.applied.len(), 1);
}

/// The module and sample of each request that reached the module.
type Applied = Vec<(usize, u64)>;

/// Plays write 1 at priority 1 met (late) at 30 and write 2 at priority 0
/// met at 64 on the control, the host weighing holds, with `held` (a
/// priority-2 write due at 40) holding it first when set. What settled,
/// and the samples requests reached the module at.
fn play_due_apart(held: bool) -> (Vec<(u64, Outcome)>, Applied) {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let holds = Holds::new(1);
    if held {
        let mut write = request(99, &AFTER);
        write.priority = 2;
        holds.take(ControlIndex(0), Standing::of(&write, 40).unwrap());
    }
    host.holds = Some(holds);
    let mut early = request(1, &AFTER);
    early.priority = 1;
    insert(&mut conditions, &slots, early, &mut store);
    let on_8 = [Predicate::Applied(RequestId(8))];
    insert(&mut conditions, &slots, request(2, &on_8), &mut store);
    for (id, at) in [(7, 30), (8, 64)] {
        store.outcomes.settle(request(id, &AFTER), Outcome::Applied { at });
    }
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    assert_eq!((conditions.len(), slots.taken()), (0, 0));
    let settled = log(&outcomes)
        .into_iter()
        .filter(|(id, _)| [1, 2].contains(id));
    (settled.collect(), host.applied)
}

/// Writes due at different samples do not contend: the holds weigh each
/// at its own. One turned away by a hold leaves the later write, due
/// after the hold, to play, though it ranks lower.
#[test]
fn a_write_a_hold_turns_away_leaves_the_later_due_one_to_play() {
    let (settled, applied) = play_due_apart(true);
    let at_64 = Outcome::Applied { at: 64 };
    assert_eq!(settled, [(1, Outcome::Superseded), (2, at_64)]);
    assert_eq!(applied, [(9, 64)]);
}

/// With no hold, the late write applies at its due sample's standing, and
/// the write due after it takes the control back, as a newer scheduled
/// write does.
#[test]
fn a_write_due_later_takes_the_control_back_from_a_late_one() {
    let (settled, applied) = play_due_apart(false);
    let late = Outcome::AppliedLate { at: 64, due: 30 };
    let at_64 = Outcome::Applied { at: 64 };
    assert_eq!(settled, [(1, late), (2, at_64)]);
    assert_eq!(applied, [(9, 64), (9, 64)]);
}
