//! What a timed edit's submitter learns of it: queued, landed, adopted with
//! the modules it built, expired, or cancelled with the reason. And what
//! other writes do to one waiting: an authored write to a module it keeps
//! is folded into its document, one to a module it rebuilds or removes
//! cancels it, a performed write never does, and so does any commit.

use super::*;
use crate::control_request::RequestId;
use crate::invention::orchestration::RuntimeSnapshot;
use crate::invention::publish::timed::EditTime;
use crate::invention::publish::{Authored, CancelCause, EditState};
use crate::{ControlValue, ControlWriteIntent};

/// Blocks before a timed edit at [`LANDING`] has landed.
const BLOCKS: usize = 6;
/// Where the rig's timed edits land, from sample 0.
const LANDING: u64 = 64 * 3 + 9;

/// The runtime's control front door over the rig.
fn front_door(rig: &Rig) -> RuntimeSnapshot {
    RuntimeSnapshot {
        state: Arc::clone(&rig.live.state),
        control_surfaces: Arc::clone(&rig.live.control_surfaces),
        event_sink: Default::default(),
        live: Some(rig.live.clone()),
    }
}

/// Queues an edit at [`LANDING`] that rebuilds `osc1` as a square wave, or
/// removes `osc2` when `remove`. As a reload or an edit batch does, its
/// document is the one prepared now, replacing whatever is retained when
/// it is adopted.
fn queue(rig: &Rig, remove: bool, ttl: Option<u64>) -> RequestId {
    let square = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "square" }),
    );
    let mut change = rig.live.begin();
    if remove {
        change.remove("osc2");
    } else {
        change.upsert("osc1", square);
    }
    let prepared = change.prepare().unwrap();
    let document = rig.live.state.lock().unwrap().document.clone();
    let retain = move |state: &mut RuntimeState| state.document = document;
    let time = EditTime::AtSample(LANDING);
    rig.live.commit_when(prepared, time, ttl, retain).unwrap()
}

fn state_of(rig: &Rig, id: RequestId) -> EditState {
    rig.live.timed_status(id).unwrap().state
}

/// `id.key` as the retained document and the module's stored config hold it.
fn recorded(rig: &Rig, id: &str, key: &str) -> (f64, f64) {
    let state = rig.live.state.lock().unwrap();
    let document = state.document.as_ref().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == id).unwrap();
    let number = |value: &serde_json::Value| value[key].as_f64().unwrap();
    (number(&spec.config), number(&state.modules[id].config))
}

fn write(rig: &Rig, id: &str, key: &str, value: f32, intent: ControlWriteIntent) {
    let value = ControlValue::Number(value);
    front_door(rig)
        .set_control_with_intent(id, key, value, intent)
        .unwrap();
}

#[test]
fn an_edit_reports_queued_landed_and_adopted_with_what_it_built() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let queued = rig.live.timed_status(id).unwrap();
    assert_eq!(
        (queued.state, queued.sample),
        (EditState::Queued, Some(LANDING))
    );

    rig.render(BLOCKS);
    // The next call taking the publisher adopts it.
    let adopted = rig.live.timed_status(id).unwrap();
    assert_eq!(adopted.state, EditState::Adopted);
    assert_eq!((adopted.sample, adopted.due), (Some(LANDING), None));
    assert_eq!(adopted.built, ["osc1"]);
    assert!(adopted.removed.is_empty());
    let changes: Vec<_> = rig.live.take_timed_changes();
    assert_eq!(changes.len(), 1, "one entry per edit, at its latest");
    assert_eq!(changes[0].state, EditState::Adopted);
    assert!(rig.live.take_timed_changes().is_empty());

    let committed = rig.live.take_timed_committed(id).unwrap();
    assert_eq!(committed.stopped, ["osc1"]);
    assert_eq!(committed.started.len(), 1);
    assert!(rig.live.take_timed_committed(id).is_none(), "taken once");
}

#[test]
fn an_edit_whose_ttl_passes_first_reports_expired() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, true, Some(64));
    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Expired);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
}

#[test]
fn any_commit_cancels_a_waiting_edit_and_says_so() {
    // A structural change, then an empty one that records a document (a
    // reload or edit batch writing controls only).
    let structural = |rig: &Rig| {
        rig.live
            .connect(edge("osc1", "audio", "osc2", "frequency_mod"))
            .unwrap();
    };
    let document = |rig: &Rig| {
        let empty = rig.live.begin().prepare().unwrap();
        rig.live.commit_with(empty, |_| {}).unwrap();
    };
    for commit in [structural, document] {
        let mut rig = Rig::new(BASE);
        let id = queue(&rig, false, None);
        commit(&rig);
        assert_eq!(
            state_of(&rig, id),
            EditState::Cancelled(CancelCause::Commit)
        );
        rig.render(BLOCKS);
        // Refused `TargetGone` once due, still reported as cancelled.
        assert_eq!(
            state_of(&rig, id),
            EditState::Cancelled(CancelCause::Commit)
        );
        let osc1 = &rig.live.state.lock().unwrap().modules["osc1"];
        assert_eq!(
            osc1.config.get("waveform"),
            Some(&serde_json::json!("sine"))
        );
    }
}

#[test]
fn an_edit_that_changes_nothing_cancels_nothing() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    rig.live.remove_module("missing").unwrap();
    assert_eq!(state_of(&rig, id), EditState::Queued);
    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
}

#[test]
fn an_authored_write_to_a_kept_module_is_folded_into_the_edit() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    write(&rig, "osc2", "frequency", 660.0, ControlWriteIntent::Author);
    assert_eq!(state_of(&rig, id), EditState::Queued);
    assert_eq!(recorded(&rig, "osc2", "frequency").0, 660.0);

    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    // The edit's own document replaced the retained one, then took the
    // write again: neither loses out.
    assert_eq!(recorded(&rig, "osc2", "frequency"), (660.0, 660.0));
    let osc1 = &rig.live.state.lock().unwrap().modules["osc1"];
    assert_eq!(
        osc1.config.get("waveform"),
        Some(&serde_json::json!("square"))
    );
}

#[test]
fn an_authored_write_to_a_module_the_edit_replaces_cancels_it() {
    for (remove, module) in [(false, "osc1"), (true, "osc2")] {
        let mut rig = Rig::new(BASE);
        let id = queue(&rig, remove, None);
        write(&rig, module, "frequency", 330.0, ControlWriteIntent::Author);
        let cause = CancelCause::AuthoredWrite {
            module_id: module.to_string(),
            key: "frequency".to_string(),
            removed: remove,
        };
        assert_eq!(state_of(&rig, id), EditState::Cancelled(cause));
        rig.render(BLOCKS);
        assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
        assert_eq!(recorded(&rig, module, "frequency"), (330.0, 330.0));
    }
}

#[test]
fn a_performed_write_never_cancels_an_edit() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    write(
        &rig,
        "osc1",
        "frequency",
        330.0,
        ControlWriteIntent::Perform,
    );
    write(
        &rig,
        "osc2",
        "frequency",
        330.0,
        ControlWriteIntent::Perform,
    );
    assert_eq!(state_of(&rig, id), EditState::Queued);
    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    assert_eq!(recorded(&rig, "osc2", "frequency").0, 550.0);
}

#[test]
fn a_refused_authored_write_leaves_the_edit_waiting() {
    let rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let door = front_door(&rig);
    let unknown = door.set_control_with_intent(
        "osc1",
        "nope",
        ControlValue::Number(1.0),
        ControlWriteIntent::Author,
    );
    assert!(unknown.is_err());
    let not_a_number = door.set_control_with_intent(
        "osc1",
        "frequency",
        ControlValue::String("loud".to_string()),
        ControlWriteIntent::Author,
    );
    assert!(not_a_number.is_err());
    assert_eq!(state_of(&rig, id), EditState::Queued);
}

#[test]
fn a_write_to_a_module_replaced_before_it_is_recorded_is_made_again() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let seen = rig.live.generation();
    let value = ControlValue::Number(330.0);
    let door = front_door(&rig);
    door.set_control_transient("osc1", "frequency", value.clone())
        .unwrap();
    // The edit lands after the write reached the oscillator it replaces.
    rig.render(BLOCKS);
    let recorded = rig.live.record_authored("osc1", "frequency", &value, seen);
    assert_eq!(
        recorded,
        Authored::Overtaken,
        "the caller writes again, to the one playing"
    );
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    let state = rig.live.state.lock().unwrap();
    let config = &state.modules["osc1"].config;
    assert_eq!(config.get("frequency"), None, "{config}");
}

#[test]
fn an_authored_write_after_the_edit_lands_reaches_the_module_playing() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    rig.render(BLOCKS);
    write(&rig, "osc1", "frequency", 330.0, ControlWriteIntent::Author);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    rig.render(1);
    let playing = rig.live.control_surfaces.lock().unwrap()["osc1"].clone();
    let frequency = playing.get_control("frequency").unwrap();
    assert_eq!(frequency, ControlValue::Number(330.0));
    // Recorded in the document; the square wave's stored config names no
    // frequency, so it stays unrecorded there.
    let state = rig.live.state.lock().unwrap();
    let document = state.document.as_ref().unwrap();
    let osc1 = document.modules.iter().find(|spec| spec.id == "osc1");
    assert_eq!(osc1.unwrap().config["frequency"], 330.0);
}

/// An edit adopted before its outcome arrives is announced adopted once:
/// the sample it landed on fills in quietly, for reads.
#[test]
fn a_landing_sample_after_adoption_is_not_announced_again() {
    use crate::control_request::Outcome;
    use crate::invention::publish::timed_status::Ledger;
    let mut ledger = Ledger::default();
    let id = RequestId(7);
    ledger.queued(id, None, None);
    ledger.adopted(id, Committed::default());
    let announced: Vec<_> = ledger.take_changes().into_iter().map(|s| s.state).collect();
    assert_eq!(announced, [EditState::Adopted]);

    ledger.outcome(id, Outcome::Applied { at: 4_096 });
    assert!(ledger.take_changes().is_empty(), "announced once");
    let read = ledger.status(id).unwrap();
    assert_eq!((read.state, read.sample), (EditState::Adopted, Some(4_096)));
}

/// Only a timed edit landing after a write makes it be written again: an
/// untimed change published meanwhile was resolved before the write
/// reached the audio thread, so the write is what plays.
#[test]
fn an_untimed_change_between_a_write_and_its_record_does_not_write_it_again() {
    let mut rig = Rig::new(BASE);
    let seen = rig.live.generation();
    let value = ControlValue::Number(330.0);
    front_door(&rig)
        .set_control_transient("osc2", "frequency", value.clone())
        .unwrap();
    let saw = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "saw" }),
    );
    let mut change = rig.live.begin();
    change.upsert("osc1", saw);
    rig.live.commit(change.prepare().unwrap()).unwrap();
    assert!(rig.live.generation() > seen);

    let recorded = rig.live.record_authored("osc2", "frequency", &value, seen);
    assert_eq!(recorded, Authored::Recorded);
    rig.render(1);
    assert_eq!(recorded_document(&rig, "osc2"), 330.0);
}

/// A timed edit landing after a write to a module it keeps leaves the
/// write playing: it is recorded, not made again.
#[test]
fn a_timed_edit_keeping_the_module_between_a_write_and_its_record_does_not_write_it_again() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let seen = rig.live.generation();
    let value = ControlValue::Number(330.0);
    front_door(&rig)
        .set_control_transient("osc2", "frequency", value.clone())
        .unwrap();
    rig.render(BLOCKS);
    let recorded = rig.live.record_authored("osc2", "frequency", &value, seen);
    assert_eq!(recorded, Authored::Recorded);
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    rig.render(1);
    let playing = rig.live.control_surfaces.lock().unwrap()["osc2"].clone();
    let frequency = playing.get_control("frequency").unwrap();
    assert_eq!(frequency, ControlValue::Number(330.0));
    assert_eq!(recorded_document(&rig, "osc2"), 330.0);
}

/// The reviewer's case: a write to `osc1` is made, a timed edit rebuilding
/// `osc1` lands after it, and making the write again is refused (the queue
/// full). Nothing is recorded, so the document still says what plays, and
/// the caller is told to make the write again.
#[test]
fn a_write_overtaken_and_refused_on_its_retry_is_not_recorded() {
    use crate::invention::orchestration::authored::{write_authored, WRITE_ATTEMPTS};
    use crate::invention::runtime::GraphCommandError;
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let live = rig.live.clone();
    let door = front_door(&rig);
    let value = ControlValue::Number(330.0);
    let mut writes = 0;
    let ended = write_authored(
        WRITE_ATTEMPTS,
        ("osc1", "frequency"),
        || live.generation(),
        || {
            writes += 1;
            if writes > 1 {
                return Err(GraphCommandError::QueueFull);
            }
            door.set_control_transient("osc1", "frequency", value.clone())?;
            rig.render(BLOCKS); // the edit lands after the write
            Ok(())
        },
        |seen| live.record_authored("osc1", "frequency", &value, seen),
    );
    let Err(GraphCommandError::ControlError(message)) = ended else {
        panic!("expected a ControlError, got {ended:?}");
    };
    assert!(message.contains("make the write again"), "{message}");
    assert_eq!(state_of(&rig, id), EditState::Adopted);
    rig.render(1);
    let playing = rig.live.control_surfaces.lock().unwrap()["osc1"].clone();
    let frequency = playing.get_control("frequency").unwrap();
    assert_eq!(frequency, ControlValue::Number(440.0), "the square plays");
    assert_eq!(recorded_document(&rig, "osc1"), 440.0, "and is recorded");
}

fn recorded_document(rig: &Rig, id: &str) -> f64 {
    let state = rig.live.state.lock().unwrap();
    let document = state.document.as_ref().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == id).unwrap();
    spec.config["frequency"].as_f64().unwrap()
}
