//! The pending store on its own: order, coalescing, holding, remapping and
//! a full store, with a recording `apply`.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{
    outcome_channel, ControlIndex, OutcomeReceiver, RequestId, RtValue, When,
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
    store.apply_due(now, INSTALLED, |target, value, _| {
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
        .map(|(id, outcome)| (id.0, outcome))
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
    store.apply_due(10, INSTALLED + 1, |target, _, _| {
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
    store.apply_due(0, INSTALLED, |_, _, _| Err(Refusal::Unsupported));
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
    let ((), allocs, frees) = allocator_events(|| {
        store.apply_due(20, INSTALLED, |target, _, _| {
            applied.push(target.control.0);
            Ok(())
        })
    });
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
