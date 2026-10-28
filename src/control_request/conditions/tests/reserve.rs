//! Reserved requests (note offs) keep a few places of their own, in the
//! table and among the slots its front doors count, that others waiting
//! cannot take.

use super::*;
use crate::control_request::{CONDITION_ROOM, WAITING_RESERVE};

#[test]
fn a_full_table_leaves_reserved_requests_their_room() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(2);
    let edge = [Predicate::edges(0, 0, 1)];
    for id in 1..=3 {
        insert(&mut conditions, &slots, request(id, &edge), &mut store);
    }
    let full = Outcome::Refused(Refusal::PendingFull);
    assert_eq!(log(&outcomes), [(3, full)]);
    let reserved = |id| {
        let mut request = request(id, &edge);
        request.reserved = true;
        request
    };
    for id in 10..10 + WAITING_RESERVE as u64 {
        insert(&mut conditions, &slots, reserved(id), &mut store);
    }
    assert!(log(&outcomes).is_empty(), "each takes the reserve");
    insert(&mut conditions, &slots, reserved(99), &mut store);
    insert(&mut conditions, &slots, request(100, &edge), &mut store);
    assert_eq!(log(&outcomes), [(99, full), (100, full)]);
    assert_eq!(conditions.len(), 2 + WAITING_RESERVE);
}

#[test]
fn the_sender_leaves_reserved_requests_their_slots() {
    let edge = [Predicate::edges(0, 0, 1)];
    let (sender, _consumer) = request_channel(4 * CONDITION_ROOM, Default::default());
    for id in 0..CONDITION_CAPACITY as u64 {
        sender.submit(request(id, &edge)).unwrap();
    }
    assert!(sender.watchers_full(false));
    for id in CONDITION_CAPACITY..CONDITION_ROOM {
        assert!(!sender.watchers_full(true), "{id}");
        let mut reserved = request(id as u64, &edge);
        reserved.reserved = true;
        sender.submit(reserved).unwrap();
    }
    assert!(sender.watchers_full(true));
}
