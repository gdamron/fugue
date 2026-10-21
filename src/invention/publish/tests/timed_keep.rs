//! An authored write to a control a timed edit writes as it lands, made
//! while the edit waits, is kept over the edit's own write: the edit lands
//! without it, and says so. What plays and what is recorded are the
//! author's value whenever the audio thread takes the write: before the
//! landing, after it, or held back by a full pending store. When it cannot
//! be kept (no room for requests waiting on conditions), the whole edit is
//! cancelled instead, as before.

use super::timed_status::{queue_writing, recorded, state_of, write, BLOCKS};
use super::*;
use crate::control_request::{
    Condition, Predicate, Request, RequestId, RequestValue, RtValue, When,
};
use crate::invention::declared::DeclaredSurface;
use crate::invention::publish::timed::EditTime;
use crate::invention::publish::{CancelCause, EditState, Touched};
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

    rig.render(BLOCKS);
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

/// A kept write queued only once the edit has landed (it landed between
/// the check and the write being queued) applies as soon as it arrives:
/// after the edit's own, so it still plays.
#[test]
fn a_kept_write_queued_after_the_edit_lands_still_plays_last() {
    let mut rig = Rig::new(BASE);
    let id = queue_writing(&rig, 440.0);
    let target = frequency_target(&rig, "osc2");
    let keeping = rig.live.publisher().lock().unwrap().timed_keeping();
    let (edit, priority) = keeping.unwrap();
    assert_eq!((edit, priority), (id, 1));
    rig.render(BLOCKS); // lands, writing 440
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(440.0)
    );

    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(660.0)));
    let condition = Condition::all(&[Predicate::Applied(edit)]).unwrap();
    request.when = When::Condition(condition);
    request.priority = priority;
    rig.live.requests.submit(request).unwrap();
    rig.render(1);
    assert_eq!(
        playing(&rig, "osc2", "frequency"),
        ControlValue::Number(660.0)
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
