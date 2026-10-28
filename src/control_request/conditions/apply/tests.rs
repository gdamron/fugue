//! At a segment start the condition table and the pending store apply as
//! one walk in time order: at one rank the write due last stays, whichever
//! table holds it, and a late event finishes before a write due after it.

use super::super::tests::{insert, log, request, store, table, Host, INSTALLED};
use super::super::{ConditionHost, Conditions};
use crate::alloc_counter::allocator_events;
use crate::control_request::condition::Predicate;
use crate::control_request::pending::{Outcome, PendingStore};
use crate::control_request::request::When;
use crate::control_request::RequestId;

/// Met as request 7 applies, at 70.
const AT_70: [Predicate; 1] = [Predicate::Applied(RequestId(7))];
/// Met as request 8 applies, at 71.
const AT_71: [Predicate; 1] = [Predicate::Applied(RequestId(8))];
/// Met as request 9 applies, at 69.
const AT_69: [Predicate; 1] = [Predicate::Applied(RequestId(9))];

/// Settles the requests the conditions wait on as applied at their samples.
fn chained(store: &mut PendingStore) {
    for (id, at) in [(7, 70), (8, 71), (9, 69)] {
        let settled = Outcome::Applied { at };
        store.outcomes.settle(request(id, &AT_70), settled);
    }
}

/// A write to the target timed at `at` in the pending store, `event` for
/// one that fires (a note off).
fn timed(store: &mut PendingStore, id: u64, at: u64, event: bool) {
    let mut timed = request(id, &AT_70);
    (timed.when, timed.event) = (When::AtSample(at), event);
    store.insert(timed, at);
}

/// Plays a segment start at `at` as the graph does: the condition table,
/// its chains, then the pending store, finishing each late event.
/// Applying allocates nothing.
fn segment_start(conditions: &mut Conditions, host: &mut Host, store: &mut PendingStore, at: u64) {
    host.segment(at);
    conditions.start(at, INSTALLED, &*host, &mut store.outcomes);
    conditions.read(INSTALLED, None, &*host, &store.outcomes.chains);
    let ((), allocs, frees) = allocator_events(|| {
        loop {
            let waiting = conditions.len();
            conditions.apply_due(INSTALLED, None, at, host, store);
            if conditions.len() == waiting {
                break;
            }
            conditions.chain(INSTALLED, &store.outcomes.chains);
        }
        let apply = |host: &mut Host, store: &mut PendingStore| {
            store.apply_due(at, INSTALLED, |target, standing, value, retirer| {
                host.apply(target, standing, value, retirer)
            })
        };
        while apply(host, store) {
            host.finish_events();
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!((conditions.len(), store.len()), (0, 0));
}

/// What settled, the chained-on requests aside.
fn settled(outcomes: &crate::control_request::OutcomeReceiver) -> Vec<(u64, Outcome)> {
    (log(outcomes).into_iter())
        .filter(|(id, _)| ![7, 8, 9].contains(id))
        .collect()
}

const fn late(due: u64) -> Outcome {
    Outcome::AppliedLate { at: 72, due }
}

/// A condition write due at 70 outranked by a pending write at 70 applies
/// after the pending write due at 69, at one rank: the control ends at
/// the write due last, not the one due earlier.
#[test]
fn a_pending_write_due_earlier_applies_before_the_writes_due_after_it() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, request(1, &AT_70), &mut store);
    timed(&mut store, 2, 69, false);
    timed(&mut store, 3, 70, false);
    chained(&mut store);
    segment_start(&mut conditions, &mut host, &mut store, 72);
    let expected = [(2, late(69)), (1, Outcome::Superseded), (3, late(70))];
    assert_eq!(settled(&outcomes), expected);
    assert_eq!(host.applied.len(), 2);
}

/// One with no rival at its sample, too: the pending write due before it
/// applies first, so the condition's write is the one that stays.
#[test]
fn a_condition_write_applies_after_a_pending_write_due_before_it() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, request(1, &AT_70), &mut store);
    timed(&mut store, 2, 69, false);
    chained(&mut store);
    segment_start(&mut conditions, &mut host, &mut store, 72);
    assert_eq!(settled(&outcomes), [(2, late(69)), (1, late(70))]);
}

/// A note off due at 69 applies and finishes before a release written for
/// 70 reaches the module: it fades under the release of its own sample.
#[test]
fn a_late_note_off_finishes_before_a_release_due_after_it() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, request(1, &AT_70), &mut store);
    timed(&mut store, 2, 69, true);
    chained(&mut store);
    segment_start(&mut conditions, &mut host, &mut store, 72);
    assert_eq!(settled(&outcomes), [(2, late(69)), (1, late(70))]);
    assert_eq!(host.finished, [1], "the note off finished alone");
}

/// Writes in the condition table apply in the order they were due, not
/// the order they were received: the one due last stays.
#[test]
fn condition_writes_apply_in_the_order_they_were_due() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, request(1, &AT_71), &mut store);
    insert(&mut conditions, &slots, request(2, &AT_69), &mut store);
    insert(&mut conditions, &slots, request(3, &AT_70), &mut store);
    chained(&mut store);
    segment_start(&mut conditions, &mut host, &mut store, 72);
    let expected = [(2, late(69)), (3, late(70)), (1, late(71))];
    assert_eq!(settled(&outcomes), expected);
}
