//! A write the condition table applies and one the pending store holds,
//! to one control at one sample: one setter runs, the higher rank's, at
//! equal rank the later submitted's (the ids say which).

use super::super::tests::{apply_all, insert, log, request, store, table, Host, INSTALLED, TARGET};
use super::super::ConditionHost;
use crate::alloc_counter::allocator_events;
use crate::control_request::condition::Predicate;
use crate::control_request::pending::Outcome;
use crate::control_request::request::When;
use crate::control_request::RequestId;

const AFTER: [Predicate; 1] = [Predicate::Applied(RequestId(7))];
const LANDED: Outcome = Outcome::Applied { at: 70 };

/// Plays condition write `cond` (waiting on request 7, applying at 70)
/// and a write timed at 70 in the pending store, with ids and priorities
/// `(cond, timed)`, as the graph applies a segment start: the condition
/// table first, then the pending store. What settled for the two, and
/// how many requests reached the module; applying allocates nothing.
fn land_together(ids: (u64, u64), priorities: (i16, i16)) -> (Vec<(u64, Outcome)>, usize) {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let mut cond = request(ids.0, &AFTER);
    cond.priority = priorities.0;
    insert(&mut conditions, &slots, cond, &mut store);
    let mut timed = request(ids.1, &AFTER);
    (timed.when, timed.priority) = (When::AtSample(70), priorities.1);
    store.insert(timed, 70);
    store.outcomes.settle(request(7, &AFTER), LANDED);
    host.segment(70);
    conditions.start(70, INSTALLED, &host, &mut store.outcomes);
    conditions.read(INSTALLED, Some(TARGET), &host, &store.outcomes.chains);
    let ((), allocs, frees) = allocator_events(|| {
        apply_all(&mut conditions, &mut host, &mut store, 70);
        store.apply_due(70, INSTALLED, |target, standing, value, retirer| {
            host.apply(target, standing, value, retirer)
        });
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!((conditions.len(), store.len(), slots.taken()), (0, 0, 0));
    let settled = log(&outcomes).into_iter().filter(|(id, _)| *id != 7);
    (settled.collect(), host.applied.len())
}

#[test]
fn the_later_submitted_plays_alone_at_equal_rank() {
    let (settled, applied) = land_together((1, 2), (0, 0));
    assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
    assert_eq!(applied, 1);
    let (settled, applied) = land_together((2, 1), (0, 0));
    assert_eq!(settled, [(1, Outcome::Superseded), (2, LANDED)]);
    assert_eq!(applied, 1);
}

#[test]
fn the_higher_rank_plays_alone_whichever_was_submitted_later() {
    let (settled, applied) = land_together((2, 1), (0, 1));
    assert_eq!(settled, [(2, Outcome::Superseded), (1, LANDED)]);
    assert_eq!(applied, 1);
    let (settled, applied) = land_together((1, 2), (1, 0));
    assert_eq!(settled, [(2, Outcome::Superseded), (1, LANDED)]);
    assert_eq!(applied, 1);
}

/// One timed at another sample is left to the holds: both apply.
#[test]
fn a_write_timed_at_another_sample_does_not_contend() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    insert(&mut conditions, &slots, request(1, &AFTER), &mut store);
    let mut timed = request(2, &AFTER);
    timed.when = When::AtSample(69);
    store.insert(timed, 69);
    store.outcomes.settle(request(7, &AFTER), LANDED);
    host.segment(70);
    conditions.start(70, INSTALLED, &host, &mut store.outcomes);
    conditions.read(INSTALLED, Some(TARGET), &host, &store.outcomes.chains);
    apply_all(&mut conditions, &mut host, &mut store, 70);
    store.apply_due(70, INSTALLED, |target, standing, value, retirer| {
        host.apply(target, standing, value, retirer)
    });
    let late = Outcome::AppliedLate { at: 70, due: 69 };
    let settled: Vec<_> = log(&outcomes)
        .into_iter()
        .filter(|(id, _)| *id != 7)
        .collect();
    assert_eq!(settled, [(1, LANDED), (2, late)]);
    assert_eq!(host.applied.len(), 2);
}
