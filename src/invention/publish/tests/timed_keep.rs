//! An authored write to a control a timed edit writes as it lands, made
//! while the edit waits, is kept over the edit's own write as the write is
//! queued, and the edit says so. Its value never sounds when the copy is
//! taken before the landing, and a drag holds one copy, not one a write.
//! What plays and what is recorded are the author's value whenever the
//! audio thread takes the write: before the landing, after it, or held
//! back by a full pending store, and the copy applying is the write's
//! own, never counted lost. When it cannot
//! be kept (no room for requests waiting on conditions, or for the copy and
//! the write both), the whole edit is cancelled instead, as before; a write
//! refused outright leaves the edit alone.

use super::timed_status::{front_door, queue_writing, recorded, state_of, write, BLOCKS, LANDING};
use super::*;
use crate::control_request::CONDITION_CAPACITY;
use crate::control_request::{
    Condition, Predicate, Request, RequestId, RequestValue, RtValue, When,
};
use crate::invention::declared::DeclaredSurface;
use crate::invention::publish::timed::EditTime;
use crate::invention::publish::{Authored, CancelCause, EditState, Touched};
use crate::{ControlValue, ControlWriteIntent};

/// What `id.key` plays.
fn playing(rig: &Rig, id: &str, key: &str) -> ControlValue {
    let surface = rig.live.control_surfaces.lock().unwrap()[id].clone();
    surface.get_control(key).unwrap()
}

/// The controls edit `id` kept authored writes to.
fn kept(rig: &Rig, id: RequestId) -> Vec<String> {
    rig.live.timed_status(id).unwrap().kept
}

/// Where a request to `id.frequency` goes.
fn frequency_target(rig: &Rig, id: &str) -> crate::control_request::ControlTarget {
    let index = {
        let surfaces = rig.live.control_surfaces.lock().unwrap();
        let surface = surfaces[id].as_any().unwrap();
        let declared = surface.downcast_ref::<DeclaredSurface>().unwrap();
        let write = declared.resolve_write("frequency", &ControlValue::Number(1.0));
        write.unwrap().index
    };
    let publisher = rig.live.publisher().lock().unwrap();
    publisher.control_target(id, index).unwrap()
}

#[test]
fn an_authored_write_to_a_control_the_edit_writes_is_kept_as_it_lands() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    rig.render(1);
    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    assert_eq!(state_of(&rig, id), EditState::Queued);
    assert_eq!(kept(&rig, id), ["osc2.frequency"]);

    // Block by block across the landing: the edit's 440 never plays.
    for block in 1..BLOCKS {
        rig.render(1);
        let at = format!("block {block}, landing in block {}", LANDING / 64);
        let frequency = playing(&rig, "osc2", "frequency");
        assert_eq!(frequency, ControlValue::Number(660.0), "{at}");
    }
    let state = state_of(&rig, id);
    rig.render(1);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(660.0)
    );
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    assert_eq!(state, EditState::Adopted, "the edit landed without it");
}

/// Several authored writes while the edit waits: the latest plays as it
/// lands, whatever order the kept writes apply in there.
#[test]
fn the_latest_of_several_kept_writes_plays() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    for frequency in [600.0, 700.0, 660.0] {
        write(
            &rig,
            "osc2",
            "frequency",
            frequency,
            ControlWriteIntent::Author,
        );
        rig.render(1);
    }
    rig.render(BLOCKS);
    let state = state_of(&rig, id);
    rig.render(1);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(660.0)
    );
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    assert_eq!(state, EditState::Adopted);
    assert_eq!(kept(&rig, id), ["osc2.frequency"]);
}

/// The edit landing between an authored write being queued and recorded:
/// kept as it was queued, the write plays through the landing, and is
/// recorded, the edit having landed without its own value sounding.
#[test]
fn a_write_kept_as_it_is_queued_plays_through_a_landing_before_its_record() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    let value = ControlValue::Number(660.0);
    let queued = front_door(&rig).make_authored("osc2", "frequency", value.clone());
    let queued = queued.unwrap();
    assert_eq!(queued.safe, Some(id), "kept over the edit");
    for block in 1..=BLOCKS {
        rig.render(1);
        let frequency = playing(&rig, "osc2", "frequency");
        assert_eq!(frequency, value, "block {block}");
    }
    let seen = (queued.generation.unwrap(), queued.safe);
    let recorded_it = rig.live.record_authored("osc2", "frequency", &value, seen);
    assert_eq!(recorded_it, Authored::Recorded);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    assert_eq!(kept(&rig, id), ["osc2.frequency"]);
}

/// An authored write queued once the edit has landed, before it is
/// adopted, follows the edit's own write: it plays, and is recorded at
/// once, as the edit it is safe from writing over it.
#[test]
fn a_write_queued_once_the_edit_has_landed_plays_and_is_recorded() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    rig.render(BLOCKS); // lands, writing 440
    let value = ControlValue::Number(660.0);
    let door = front_door(&rig);
    let queued = door.make_authored("osc2", "frequency", value.clone());
    let queued = queued.unwrap();
    assert_eq!(queued.safe, Some(id), "queued after the landing");
    let seen = queued.generation.unwrap();
    let unsafe_record = rig
        .live
        .record_authored("osc2", "frequency", &value, (seen, None));
    assert_eq!(
        unsafe_record,
        Authored::Overtaken,
        "as were it queued before"
    );
    let recorded_it = rig
        .live
        .record_authored("osc2", "frequency", &value, (seen, queued.safe));
    assert_eq!(recorded_it, Authored::Recorded);
    rig.render(1);
    assert_eq!(playing(&rig, "osc2", "frequency"), value);
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    assert!(kept(&rig, id).is_empty(), "nothing to keep it over");
}

/// A kept write holds at the author's own rank: a request chained to the
/// edit after it has landed, and resolving to the landing sample, still
/// plays, as it would were nothing kept.
#[test]
fn a_kept_write_refuses_no_later_request_chained_to_the_landing() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    assert_eq!(kept(&rig, id), ["osc2.frequency"]);
    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    let mut request = Request::new(
        frequency_target(&rig, "osc2"),
        RequestValue::Value(RtValue::F32(700.0)),
    );
    let landed = Condition::all(&[Predicate::Applied(id)]).unwrap();
    request.when = When::Condition(landed);
    rig.live.requests.submit(request).unwrap();
    rig.render(2);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(700.0)
    );
}

/// However late the audio thread takes the write: here the pending store
/// is full through the landing sample, so the write plays after the
/// edit's own value. What plays and what is recorded are the write.
#[test]
fn a_kept_write_held_back_past_the_landing_still_plays() {
    let mut rig = Rig::new(BASE);
    let landing: u64 = 64 * 10 + 9;
    let square = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "square" }),
    );
    let mut change = rig.live.begin();
    change.upsert("osc1", square);
    let prepared = change.prepare().unwrap();
    // As an edit batch's retained document: osc2.frequency becomes 440.
    let mut document = rig.live.state.lock().unwrap().document.clone().unwrap();
    for spec in &mut document.modules {
        if spec.id == "osc2" {
            spec.config["frequency"] = serde_json::json!(440.0);
        }
    }
    let retain = move |state: &mut RuntimeState| state.document = Some(document);
    let chained = (
        "osc2".to_string(),
        "frequency".to_string(),
        ControlValue::Number(440.0),
    );
    let time = EditTime::AtSample(landing);
    let id = rig
        .live
        .commit_built_when(prepared, None, time, None, retain, &[chained])
        .unwrap();
    rig.render(1);
    // Fill the pending store with writes to osc1 (which the edit rebuilds,
    // so its landing refuses them all and frees the store).
    let target = frequency_target(&rig, "osc1");
    let mut n = 0u64;
    for _ in 0..3 {
        for _ in 0..255 {
            let mut request = Request::new(target, RequestValue::Value(RtValue::F32(300.0)));
            request.when = When::AtSample(100_000 + n);
            n += 1;
            if rig.live.requests.submit(request).is_err() {
                break;
            }
        }
        rig.render(1);
    }
    assert_eq!(state_of(&rig, id), EditState::Queued);

    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    assert_eq!(kept(&rig, id), ["osc2.frequency"]);
    // Read through the runtime first, as a caller would, so whatever
    // landed is adopted: what is recorded and what plays then fail on
    // their own if they differ.
    rig.render(20);
    let state = state_of(&rig, id);
    let (document, _) = recorded(&rig, "osc2", "frequency");
    assert_eq!(document, 660.0);
    rig.render(1_600);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(660.0)
    );
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    assert_eq!(state, EditState::Adopted);
}

/// With no room for another request waiting on a condition, the write
/// cannot be kept: the whole edit is cancelled, and says why.
#[test]
fn a_write_that_cannot_be_kept_cancels_the_edit() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    let target = frequency_target(&rig, "osc1");
    let never = Condition::all(&[Predicate::Applied(RequestId(u64::MAX))]).unwrap();
    while !rig.live.requests.watch_slots().full() {
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(300.0)));
        request.when = When::Condition(never);
        rig.live.requests.submit(request).unwrap();
    }

    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    rig.render(BLOCKS);
    let state = state_of(&rig, id);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(660.0)
    );
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    let cause = CancelCause::AuthoredWrite {
        module_id: "osc2".to_string(),
        key: "frequency".to_string(),
        touches: Touched::Written,
    };
    assert_eq!(state, EditState::Cancelled(cause));
    assert!(kept(&rig, id).is_empty());
}

/// Fills the queue through osc1's surface, as performed writes do, and
/// says how many it took: all it can, or `count`.
fn fill(rig: &Rig, count: Option<usize>) -> usize {
    let door = front_door(rig);
    let mut made = 0;
    while count.is_none_or(|count| made < count) {
        let value = ControlValue::Number(300.0);
        if door
            .set_control_transient("osc1", "frequency", value)
            .is_err()
        {
            break;
        }
        made += 1;
    }
    made
}

/// With no room for the write, it is refused and the edit is left alone,
/// with nothing kept; with room for the write and not a copy too, it is
/// made, not kept, and the edit is cancelled. Never a copy without the
/// write.
#[test]
fn a_write_with_no_room_for_a_copy_is_not_kept() {
    let mut full = Rig::new(BASE);
    let id = queue_writing(&full, 440.0);
    let room = fill(&full, None);
    let value = ControlValue::Number(660.0);
    let door = front_door(&full);
    let refused = door.set_control_with_intent(
        "osc2",
        "frequency",
        value.clone(),
        ControlWriteIntent::Author,
    );
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(state_of(&full, id), EditState::Queued, "left alone");
    assert!(kept(&full, id).is_empty());
    full.render(BLOCKS);
    assert_eq!(state_of(&full, id), EditState::Adopted);
    full.render(1);
    let edits = ControlValue::Number(440.0);
    assert_eq!(playing(&full, "osc2", "frequency"), edits);
    let (document, _) = recorded(&full, "osc2", "frequency");
    assert_ne!(document, 660.0, "the refused write is not recorded");

    let mut one = Rig::new(BASE);
    let id = queue_writing(&one, 440.0);
    assert_eq!(fill(&one, Some(room - 1)), room - 1);
    write(&one, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    assert!(kept(&one, id).is_empty());
    let cause = CancelCause::AuthoredWrite {
        module_id: "osc2".to_string(),
        key: "frequency".to_string(),
        touches: Touched::Written,
    };
    assert_eq!(state_of(&one, id), EditState::Cancelled(cause));
    one.render(BLOCKS);
    assert_eq!(playing(&one, "osc2", "frequency"), value);
    assert_eq!(recorded(&one, "osc2", "frequency"), (660.0, 660.0));
}

/// As [`queue_writing`], landing a hundred blocks ahead.
fn queue_writing_far(rig: &Rig, frequency: f32) -> RequestId {
    let square = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "square" }),
    );
    let mut change = rig.live.begin();
    change.upsert("osc1", square);
    let prepared = change.prepare().unwrap();
    let document = rig.live.state.lock().unwrap().document.clone();
    let retain = move |state: &mut RuntimeState| state.document = document;
    let time = EditTime::AtSample(64 * 100 + 9);
    let frequency = ControlValue::Number(frequency);
    let write = ("osc2".to_string(), "frequency".to_string(), frequency);
    rig.live
        .commit_built_when(prepared, None, time, None, retain, &[write])
        .unwrap()
}

/// A slider dragged over a control the edit writes: each write's copy
/// replaces the last, so the drag holds one slot however long it runs, and
/// the edit waits on, landing with the drag's last value.
#[test]
fn a_drag_on_a_kept_control_holds_one_slot_and_keeps_the_edit() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing_far(&rig, 440.0);
    let author = ControlWriteIntent::Author;
    let drag = CONDITION_CAPACITY + 16;
    for i in 0..drag {
        write(&rig, "osc2", "frequency", 500.0 + i as f32, author);
        rig.render(1);
        assert_eq!(state_of(&rig, id), EditState::Queued, "after write {i}");
        assert_eq!(rig.live.requests.watch_slots().taken(), 1, "write {i}");
    }
    rig.render(110 - drag);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    let last = 500.0 + (drag - 1) as f32;
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(last)
    );
    assert_eq!(
        recorded(&rig, "osc2", "frequency"),
        (last as f64, last as f64)
    );
}

/// The copy applying at the landing is logged as the write's, not lost.
#[test]
fn a_kept_copy_is_not_counted_lost() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing_far(&rig, 440.0);
    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    rig.render(110);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    let log = rig.live.request_log();
    assert_eq!(log.lost, 0, "{log:?}");
}
