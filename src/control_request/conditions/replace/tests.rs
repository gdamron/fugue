//! A request waiting on another applying replacing one waiting for the
//! same control on the same request, by rank, and nothing else replacing.

use super::super::tests::{insert, log, request, store, table};
use crate::control_request::condition::Predicate;
use crate::control_request::pending::Outcome;
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
