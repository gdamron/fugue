//! A value replacing others, as its config key declares (a tape's notes,
//! which rewrite its gain): writing it forgets what it replaces from the
//! saved document and the reload baseline, and a reload writes it before
//! every key it replaces, changed or not, so the module plays what a cold
//! build of the document plays, reload after reload.

use serde_json::{json, Value};

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::test_support::tape::TapeFactory;

fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::default();
    registry.register(TapeFactory);
    registry
}

fn tape(config: Value) -> Invention {
    let document = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "tape", "type": "tape", "config": config },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "tape", "from_port": "out", "to": "dac", "to_port": "audio" }
        ]
    });
    doc(&document.to_string())
}

fn start_tape(config: Value) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry())
        .build(tape(config))
        .unwrap();
    start_manual(runtime)
}

/// Notes of `[2]` carrying `gain`, as JSON text.
fn carrying(gain: f32) -> String {
    format!(r#"{{"notes": [2], "gain": {gain}}}"#)
}

/// The retained document as saving writes it and loading reads it back.
fn saved(running: &RunningInvention) -> Invention {
    let json = running.document().unwrap().to_json().unwrap();
    Invention::from_json(&json).unwrap()
}

fn gain(running: &RunningInvention) -> ControlValue {
    running.get_control("tape", "gain").unwrap()
}

/// The gain a cold build of `document`'s tape plays at.
fn built_gain(document: &Invention) -> ControlValue {
    let built = registry().build("tape", SAMPLE_RATE, &document.modules[0].config);
    let surface = built.unwrap().control_surface.unwrap();
    surface.get_control("gain").unwrap()
}

#[test]
fn a_reload_changing_only_a_payload_plays_what_a_cold_build_plays() {
    let (mut running, pump) = start_tape(json!({ "gain": 0.5, "notes": [1] }));
    let edited = tape(json!({ "gain": 0.5, "notes": carrying(0.25) }));
    let report = running.reload(edited.clone()).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    // The gain is unchanged, and written again after the notes.
    assert_eq!(report.controls_updated, ["tape.notes", "tape.gain"]);
    assert_eq!(gain(&running), built_gain(&edited));
    assert_eq!(gain(&running), 0.5.into());

    let again = running.reload(edited).expect("diff applies");
    pump.render(1);
    assert!(again.controls_updated.is_empty(), "{again:?}");
    assert_eq!(gain(&running), 0.5.into());
}

#[test]
fn a_reload_writes_a_payload_before_the_scalars_beside_it() {
    let (mut running, pump) = start_tape(json!({ "notes": [1] }));
    // The scalar's key sorts before the payload's.
    let edited = tape(json!({ "gain": 0.5, "notes": carrying(0.25) }));
    let report = running.reload(edited.clone()).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["tape.notes", "tape.gain"]);
    assert_eq!(gain(&running), built_gain(&edited));
}

#[test]
fn a_written_payload_forgets_what_it_replaces_in_the_document_and_the_baseline() {
    // Notes as the JSON text a control takes: an array is reloaded by a
    // rebuild.
    let original = json!({ "gain": 0.5, "notes": "[1]" });
    let (mut running, pump) = start_tape(original.clone());
    running
        .set_control("tape", "notes", carrying(0.25).into())
        .unwrap();
    pump.render(1);
    assert_eq!(gain(&running), 0.25.into());
    let document = saved(&running);
    assert_eq!(document.modules[0].config.get("gain"), None);
    assert_eq!(built_gain(&document), 0.25.into());

    let report = running.reload(document).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_eq!(gain(&running), 0.25.into());

    // The original brings back the gain the notes replaced.
    let report = running.reload(tape(original)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["tape.notes", "tape.gain"]);
    assert_eq!(gain(&running), 0.5.into());
}

#[test]
fn a_scalar_written_after_a_payload_is_kept_beside_it() {
    let original = json!({ "gain": 0.5, "notes": "[1]" });
    let (mut running, pump) = start_tape(original.clone());
    running
        .set_control("tape", "notes", carrying(0.25).into())
        .unwrap();
    running.set_control("tape", "gain", 0.75.into()).unwrap();
    pump.render(1);
    let document = saved(&running);
    assert_eq!(document.modules[0].config["gain"], json!(0.75));
    assert_eq!(built_gain(&document), 0.75.into());
    let report = running.reload(document).expect("diff applies");
    pump.render(1);
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_eq!(gain(&running), 0.75.into());

    // The document saved between the two writes plays the notes' gain: the
    // removed gain returns to it by writing the notes again, as a fresh
    // build of that document holds it.
    let between = tape(json!({ "notes": carrying(0.25) }));
    let report = running.reload(between.clone()).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["tape.notes"], "{report:?}");
    assert_eq!(gain(&running), built_gain(&between));
    assert_eq!(gain(&running), 0.25.into());

    let report = running.reload(tape(original)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(gain(&running), 0.5.into());
}

#[test]
fn a_write_within_a_payload_the_document_holds_is_restored_by_reloading_it() {
    let original = json!({ "notes": carrying(0.25) });
    let (mut running, pump) = start_tape(original.clone());
    running.set_control("tape", "gain", 0.75.into()).unwrap();
    pump.render(1);
    // As a write to any key the document leaves implicit: restored to what
    // the payload builds, unrebuilt.
    let report = running.reload(tape(original)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(gain(&running), 0.25.into());
}

#[test]
fn a_reload_of_the_saved_document_takes_a_written_payload_as_the_documents() {
    let (mut running, pump) = start_tape(json!({ "notes": carrying(0.25) }));
    running
        .set_control("tape", "notes", carrying(0.5).into())
        .unwrap();
    pump.render(1);
    // Reloading the document saved after the notes changes nothing, and
    // from then on the document holds them: a later write within them is
    // restored by reloading it, as on a fresh load of that document.
    let document = saved(&running);
    running.reload(document.clone()).expect("diff applies");
    running.set_control("tape", "gain", 0.75.into()).unwrap();
    pump.render(1);
    let report = running.reload(document).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(gain(&running), 0.5.into());
}

#[test]
fn a_reload_replaying_a_key_no_control_takes_rebuilds() {
    let (mut running, pump) = start_tape(json!({ "gain": [0.5], "notes": "[1]" }));
    let edited = tape(json!({ "gain": [0.5], "notes": carrying(0.25) }));
    let report = running.reload(edited.clone()).expect("diff applies");
    pump.render(1);
    assert_eq!(report.swapped, ["tape"], "{report:?}");
    assert_eq!(gain(&running), built_gain(&edited));
}
