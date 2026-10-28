//! A request waiting on another applying replacing one waiting for the
//! same control on the same request, by rank, and nothing else replacing.

use super::super::tests::{insert, log, play_ordered, request, store, table, Host};
use crate::alloc_counter::allocator_events;
use crate::control_request::condition::{Condition, Predicate};
use crate::control_request::pending::{Outcome, Refusal};
use crate::control_request::request::{Request, When};
use crate::control_request::RequestId;

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
/// first) and the edit's own `write` waiting beside it (queued first, or
/// after the copy when `copy_first`): what settled, and how many requests
/// reached the module.
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

/// As the edit lands, its own write gives way to the copy before either
/// reaches the module, so one setter runs (a reverb retunes once).
#[test]
fn an_edits_write_gives_way_to_the_copy_landing_with_it() {
    for copy_first in [false, true] {
        let (settled, applied) = land_beside(request(1, &AFTER), copy_first);
        let landed = Outcome::Applied { at: 70 };
        assert_eq!(settled, [(1, Outcome::Superseded), (2, landed)]);
        assert_eq!(applied, 1);
    }
}

/// An edit's write that outranks the copy plays, and the copy gives way.
#[test]
fn an_edits_write_outranking_the_copy_plays_in_its_place() {
    for copy_first in [false, true] {
        let mut write = request(1, &AFTER);
        write.priority = 1;
        let (settled, applied) = land_beside(write, copy_first);
        let landed = Outcome::Applied { at: 70 };
        assert_eq!(settled, [(2, Outcome::Superseded), (1, landed)]);
        assert_eq!(applied, 1);
    }
}
