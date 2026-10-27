//! The pending store on its own: order, coalescing, holding, remapping and
//! a full store, with a recording `apply`.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{
    outcome_channel, outcome_channel_reserving, ControlIndex, OutcomeReceiver, RequestId, RtValue,
    When,
};

const INSTALLED: u64 = 2;

fn target(generation: u64, module_idx: usize, control: u16) -> ControlTarget {
    ControlTarget {
        generation,
        module_idx,
        control: ControlIndex(control),
    }
}

/// A store for `capacity` requests whose retirer holds a few payloads,
/// and the receiving end of its outcomes.
pub(super) fn store_of(capacity: usize) -> (PendingStore, OutcomeReceiver) {
    let queue = crate::payload::RetireQueue::with_capacity(4);
    let (sender, outcomes) = outcome_channel(4096);
    let store = PendingStore::new(capacity, Retirer::new(queue, 8), sender);
    (store, outcomes)
}

/// Request `id` writing `value` to `target`.
fn request(id: u64, target: ControlTarget, value: f32) -> Request {
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.id = RequestId(id);
    request
}

/// Applies everything due at `now`, returning `(module, control, value)` in
/// the order applied.
fn apply_at(store: &mut PendingStore, now: u64) -> Vec<(usize, u16, f32)> {
    let mut applied = Vec::new();
    store.apply_due(now, INSTALLED, |target, _, value, _| {
        let RequestValue::Value(RtValue::F32(value)) = value else {
            panic!("unexpected value {value:?}");
        };
        applied.push((target.module_idx, target.control.0, value));
        Ok(())
    });
    applied
}

fn log(outcomes: &OutcomeReceiver) -> Vec<(u64, Outcome)> {
    std::iter::from_fn(|| outcomes.try_recv())
        .map(|(id, outcome, _)| (id.0, outcome))
        .collect()
}

#[test]
fn the_same_target_at_the_same_time_coalesces_last_wins() {
    let (mut store, outcomes) = store_of(8);
    let a = target(INSTALLED, 0, 0);
    store.insert(request(1, a, 1.0), 10);
    store.insert(request(2, target(INSTALLED, 0, 1), 2.0), 10);
    store.insert(request(3, a, 3.0), 10);
    // A different time or another generation does not coalesce.
    store.insert(request(4, a, 4.0), 11);
    store.insert(request(5, target(INSTALLED + 1, 0, 0), 5.0), 10);
    assert_eq!(store.len(), 4);
    assert_eq!(log(&outcomes), [(1, Outcome::Superseded)]);

    // The replacement takes the latest receipt position at its time.
    assert_eq!(apply_at(&mut store, 10), [(0, 1, 2.0), (0, 0, 3.0)]);
    assert_eq!(apply_at(&mut store, 11), [(0, 0, 4.0)]);
}

#[test]
fn a_timed_request_s_outcome_takes_the_room_kept_for_it() {
    let queue = crate::payload::RetireQueue::with_capacity(4);
    // Four slots, one of them kept for timed requests' outcomes.
    let (sender, outcomes) = outcome_channel_reserving(3, 1);
    let mut store = PendingStore::new(8, Retirer::new(queue, 8), sender);
    for n in 1..=4 {
        store.insert(request(n, target(INSTALLED, 0, n as u16), 0.0), 10);
    }
    let mut timed = request(5, target(INSTALLED, 1, 0), 0.0);
    timed.when = When::AtSample(10);
    store.insert(timed, 10);
    assert_eq!(apply_at(&mut store, 10).len(), 5);
    let received: Vec<_> = log(&outcomes).into_iter().map(|(id, _)| id).collect();
    assert_eq!(received, [1, 2, 3, 5], "the fourth untimed one dropped");
}

#[test]
fn a_full_store_refuses_and_never_grows() {
    let (mut store, outcomes) = store_of(4);
    let capacity = store.capacity();
    for n in 0..capacity as u64 {
        store.insert(request(n, target(INSTALLED, 0, n as u16), 0.0), 100);
    }
    let ((), allocs, frees) = allocator_events(|| {
        store.insert(request(90, target(INSTALLED, 0, 90), 0.0), 50);
        // Coalescing still works when full: it does not grow the store.
        store.insert(request(91, target(INSTALLED, 0, 0), 1.0), 100);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(store.len(), capacity);
    assert_eq!(store.capacity(), capacity);
    assert_eq!(
        log(&outcomes),
        [
            (90, Outcome::Refused(Refusal::PendingFull)),
            (0, Outcome::Superseded),
        ]
    );
}

#[test]
fn held_entries_neither_apply_nor_bound_a_segment() {
    let (mut store, _outcomes) = store_of(8);
    store.insert(request(1, target(INSTALLED + 1, 0, 0), 1.0), 5);
    store.insert(request(2, target(INSTALLED, 0, 0), 2.0), 20);
    assert_eq!(store.next_due(INSTALLED), Some(20));
    assert_eq!(apply_at(&mut store, 10), []);
    assert_eq!(store.len(), 2);

    // Once its generation installs it is due (late) at once.
    let mut applied = Vec::new();
    store.apply_due(10, INSTALLED + 1, |target, _, _, _| {
        applied.push(target.module_idx);
        Ok(())
    });
    assert_eq!(applied, [0]);
}

#[test]
fn remapping_rewrites_or_refuses_older_entries_only() {
    let (mut store, outcomes) = store_of(8);
    store.insert(request(1, target(INSTALLED - 1, 3, 0), 1.0), 10);
    store.insert(request(2, target(INSTALLED - 1, 4, 0), 2.0), 10);
    store.insert(request(3, target(INSTALLED + 1, 4, 0), 3.0), 10);
    store.insert(request(4, target(INSTALLED, 4, 0), 4.0), 10);
    // Module 3 moved to 1; module 4 went away.
    store.remap(INSTALLED, 0, |target| (target.module_idx == 3).then_some(1));
    assert_eq!(log(&outcomes), [(2, Outcome::Refused(Refusal::TargetGone))]);
    assert_eq!(apply_at(&mut store, 10), [(1, 0, 1.0), (4, 0, 4.0)]);
    assert_eq!(store.len(), 1, "the held entry stays");
}

#[test]
fn a_refused_apply_settles_refused() {
    let (mut store, outcomes) = store_of(2);
    store.insert(request(1, target(INSTALLED, 0, 0), 1.0), 0);
    store.apply_due(0, INSTALLED, |_, _, _, _| Err(Refusal::Unsupported.into()));
    assert_eq!(store.len(), 0);
    assert_eq!(
        log(&outcomes),
        [(1, Outcome::Refused(Refusal::Unsupported))]
    );
}

/// A payload request to `target`, taken by the intake (room reserved).
fn take_payload(store: &mut PendingStore, id: u64, target: ControlTarget, at: u64) {
    let payload = RequestValue::Payload(crate::payload::Payload::new(id));
    let mut request = Request::new(target, payload);
    request.id = RequestId(id);
    assert!(store.outcomes.has_room_for_payload());
    store.outcomes.reserve(&request);
    store.insert(request, at);
}

#[test]
fn pending_payloads_reserve_retire_room_until_they_settle() {
    // The retirer holds 8: room for 4 payloads at 2 retirements each.
    let (mut store, outcomes) = store_of(8);
    for id in 0..4 {
        take_payload(&mut store, id, target(INSTALLED, id as usize, 0), 10);
    }
    assert!(!store.outcomes.has_room_for_payload());
    // Plain values reserve nothing.
    store.insert(request(9, target(INSTALLED, 9, 0), 1.0), 10);
    assert!(!store.outcomes.has_room_for_payload());
    // Superseding one, or refusing one, retires it and releases its room.
    store.insert(request(5, target(INSTALLED, 0, 0), 2.0), 10);
    let gone = |t: &ControlTarget| (t.module_idx != 1).then_some(t.module_idx);
    store.remap(INSTALLED + 1, 0, gone);
    assert!(store.outcomes.has_room_for_payload());
    take_payload(&mut store, 6, target(INSTALLED + 1, 6, 0), 10);
    assert_eq!(
        log(&outcomes),
        [
            (0, Outcome::Superseded),
            (1, Outcome::Refused(Refusal::TargetGone))
        ]
    );
}

#[test]
fn an_entry_applied_past_its_due_sample_is_late_and_past_its_expiry_is_refused() {
    let (mut store, outcomes) = store_of(8);
    let mut expiring = request(1, target(INSTALLED, 0, 0), 1.0);
    expiring.expires = Some(15);
    store.insert(expiring, 10);
    let mut lasting = request(2, target(INSTALLED, 0, 1), 2.0);
    lasting.expires = Some(20);
    store.insert(lasting, 10);
    store.insert(request(3, target(INSTALLED, 0, 2), 3.0), 20);

    // Applied at 20: the first expired at 15, the second is late but still
    // in time, the third is exactly on time.
    let mut applied = Vec::with_capacity(8);
    let (stopped, allocs, frees) = allocator_events(|| {
        store.apply_due(20, INSTALLED, |target, _, _, _| {
            applied.push(target.control.0);
            Ok(())
        })
    });
    assert!(!stopped);
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(applied, [1, 2]);
    assert_eq!(
        log(&outcomes),
        [
            (1, Outcome::Refused(Refusal::Expired)),
            (2, Outcome::AppliedLate { at: 20, due: 10 }),
            (3, Outcome::Applied { at: 20 }),
        ]
    );
    assert_eq!(store.len(), 0);
}

#[test]
fn inserting_beside_replaces_nothing_and_both_apply_in_receipt_order() {
    let (mut store, outcomes) = store_of(8);
    let a = target(INSTALLED, 0, 0);
    store.insert(request(1, a, 1.0), 10);
    store.insert_beside(request(2, a, 2.0), 10);
    assert_eq!(store.len(), 2);
    assert_eq!(log(&outcomes), []);
    assert_eq!(apply_at(&mut store, 10), [(0, 0, 1.0), (0, 0, 2.0)]);
}

#[test]
fn edits_never_coalesce_and_a_refused_one_retires_its_publication() {
    let (mut store, outcomes) = store_of(8);
    let edit = |id| {
        let mut edit = Request::edit(INSTALLED, crate::payload::Payload::owned(Box::new(id)));
        edit.id = RequestId(id);
        edit
    };
    for id in [1, 2] {
        let edit = edit(id);
        assert!(edit.value.is_edit() && edit.value.is_payload());
        assert_eq!(edit.when, When::Now);
        assert!(store.outcomes.has_room_for_payload());
        store.outcomes.reserve(&edit);
        store.insert(edit, 10);
    }
    assert_eq!(store.len(), 2, "two edits at one sample are two edits");
    // Settled refused (their generation went away), each retires its
    // publication and releases its room.
    let ((), allocs, frees) = allocator_events(|| store.remap(INSTALLED + 1, 0, |_| None));
    assert_eq!((allocs, frees), (0, 0));
    let gone = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(log(&outcomes), [(1, gone), (2, gone)]);
    assert_eq!(store.len(), 0);
    assert!(store.outcomes.has_room_for_payload());
    assert_eq!(
        store.outcomes.retirer.held(),
        0,
        "both went to the retire queue"
    );
}

/// Request `id` firing an event on `target` with `value`.
fn event(id: u64, target: ControlTarget, value: f32) -> Request {
    Request {
        event: true,
        ..request(id, target, value)
    }
}

#[test]
fn at_one_sample_values_apply_before_events_whatever_the_receipt_order() {
    let (mut store, _outcomes) = store_of(8);
    store.insert(event(1, target(INSTALLED, 0, 2), 60.0), 10);
    store.insert(request(2, target(INSTALLED, 0, 0), 4.0), 10);
    store.insert(event(3, target(INSTALLED, 0, 2), 64.0), 10);
    store.insert(request(4, target(INSTALLED, 1, 0), 0.5), 10);
    let applied = apply_at(&mut store, 10);
    assert_eq!(
        applied,
        [(0, 0, 4.0), (1, 0, 0.5), (0, 2, 60.0), (0, 2, 64.0)]
    );
}

#[test]
fn writes_due_at_different_samples_apply_in_time_order() {
    let (mut store, _outcomes) = store_of(8);
    store.insert(request(1, target(INSTALLED, 0, 0), 4.0), 8);
    store.insert(event(2, target(INSTALLED, 0, 2), 60.0), 5);
    // A held entry between them is skipped and stays.
    store.insert(request(3, target(INSTALLED + 1, 0, 0), 1.0), 5);
    store.insert(event(4, target(INSTALLED, 0, 2), 64.0), 8);
    // Late at 10: the store stops behind each late event, for its caller
    // to finish it.
    let mut applied = Vec::new();
    let mut apply = |store: &mut PendingStore| {
        store.apply_due(10, INSTALLED, |target, _, value, _| {
            let RequestValue::Value(RtValue::F32(value)) = value else {
                panic!("unexpected value {value:?}");
            };
            applied.push((target.module_idx, target.control.0, value));
            Ok(())
        })
    };
    assert!(apply(&mut store), "stopped behind the event due at 5");
    assert!(apply(&mut store), "stopped behind the event due at 8");
    assert!(!apply(&mut store));
    assert_eq!(applied, [(0, 2, 60.0), (0, 0, 4.0), (0, 2, 64.0)]);
    assert_eq!(store.len(), 1, "the held entry stays");
}

#[test]
fn only_a_late_event_that_applies_stops_the_store() {
    let (mut store, _outcomes) = store_of(8);
    // Late values alone apply together.
    store.insert(request(1, target(INSTALLED, 0, 0), 1.0), 4);
    store.insert(request(2, target(INSTALLED, 0, 1), 2.0), 6);
    // So do the writes after a late event that expired or was refused.
    let mut expired = event(3, target(INSTALLED, 0, 2), 60.0);
    expired.expires = Some(9);
    store.insert(expired, 7);
    store.insert(event(4, target(INSTALLED, 1, 2), 62.0), 8);
    store.insert(request(5, target(INSTALLED, 0, 1), 3.0), 9);
    let mut applied = Vec::new();
    let stopped = store.apply_due(10, INSTALLED, |target, _, _, _| {
        applied.push(target.module_idx);
        match target.module_idx {
            1 => Err(Refusal::Unsupported.into()),
            _ => Ok(()),
        }
    });
    assert!(!stopped);
    assert_eq!(applied, [0, 0, 1, 0]);
    assert_eq!(store.len(), 0);
}

#[test]
fn late_events_at_one_sample_stop_the_store_one_by_one() {
    let (mut store, _outcomes) = store_of(8);
    store.insert(event(1, target(INSTALLED, 0, 2), 60.0), 4);
    store.insert(event(2, target(INSTALLED, 1, 2), 62.0), 4);
    store.insert(request(3, target(INSTALLED, 2, 0), 1.0), 4);
    let mut applied = Vec::new();
    let mut stops = Vec::new();
    for _ in 0..3 {
        stops.push(store.apply_due(10, INSTALLED, |target, _, _, _| {
            applied.push(target.module_idx);
            Ok(())
        }));
    }
    // The value first, then each event, finished before the next applies.
    assert_eq!(stops, [true, true, false]);
    assert_eq!(applied, [2, 0, 1]);
    assert_eq!(store.len(), 0);
}

#[test]
fn a_reserved_request_takes_the_place_of_the_latest_due_one_that_is_not() {
    let (mut store, outcomes) = store_of(4);
    let capacity = store.capacity();
    let reserved = |id: u64| {
        let mut request = request(id, target(INSTALLED, 0, id as u16), 0.0);
        request.reserved = true;
        request
    };
    for n in 0..4 {
        store.insert(request(n, target(INSTALLED, 0, n as u16), 0.0), 100 + n);
    }
    let ((), allocs, frees) = allocator_events(|| {
        // At the latest one's own sample, then earlier.
        store.insert(reserved(90), 103);
        for id in 91..=93 {
            store.insert(reserved(id), 10);
        }
        // Every one waiting is reserved: neither kind finds a place.
        store.insert(reserved(94), 10);
        store.insert(request(95, target(INSTALLED, 0, 95), 0.0), 10);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!((store.len(), store.capacity()), (4, capacity));
    let refused = |id| (id, Outcome::Refused(Refusal::PendingFull));
    let expected = [3, 2, 1, 0, 94, 95].map(refused);
    assert_eq!(log(&outcomes), expected);
    let applied: Vec<_> = apply_at(&mut store, 200).iter().map(|e| e.1).collect();
    assert_eq!(applied, [91, 92, 93, 90], "in time, then receipt, order");
}
