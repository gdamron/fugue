//! Reloading a melody after a whole scale: the stored config forgets what
//! the scale replaced, as the saved document does, and a whole scale
//! applies before the degrees written within it, on reload and when a
//! development's config is applied.

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

/// A melody whose scale is written as JSON text, as a document saved
/// after a whole-scale write holds it.
const MELODY: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "tune", "type": "melody",
          "config": { "degrees": "[0, 2, 4]", "degree_count": 3, "seed": 42 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "tune", "from_port": "frequency", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn start_pumped(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    start_manual(runtime)
}

/// The retained document as saving writes it and loading reads it back.
fn saved(running: &RunningInvention) -> Invention {
    let json = running.document().unwrap().to_json().unwrap();
    Invention::from_json(&json).unwrap()
}

fn read(running: &RunningInvention, id: &str, key: &str) -> ControlValue {
    running.get_control(id, key).unwrap()
}

/// A whole scale, then a degree written within it.
fn rescored(json: &str) -> (RunningInvention, Pump) {
    let (running, pump) = start_pumped(json);
    running
        .set_control("tune", "degrees", ControlValue::String("[0, 5]".into()))
        .unwrap();
    running
        .set_control("tune", "degree.1", number(4.0))
        .unwrap();
    pump.render(1);
    (running, pump)
}

#[test]
fn reloading_the_document_saved_after_a_whole_scale_rebuilds_nothing() {
    let (mut running, pump) = rescored(MELODY);
    let report = running.reload(saved(&running)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(read(&running, "tune", "degree_count"), number(2.0));
    assert_eq!(read(&running, "tune", "degree.0"), number(0.0));
    assert_eq!(read(&running, "tune", "degree.1"), number(4.0));
}

#[test]
fn reloading_the_original_after_a_whole_scale_brings_back_what_it_replaced() {
    let (mut running, pump) = rescored(MELODY);
    let report = running.reload(doc(MELODY)).expect("diff applies");
    pump.render(1);
    // The degree written within the scale is gone from the original, and a
    // removed key rebuilds the melody as the original builds.
    assert_eq!(report.swapped, ["tune"], "{report:?}");
    assert_eq!(read(&running, "tune", "degree_count"), number(3.0));
    assert_eq!(read(&running, "tune", "degree.1"), number(2.0));
}

#[test]
fn a_live_count_within_a_loaded_scale_survives_a_reload_leaving_it_be() {
    let loaded = MELODY.replace(r#" "degree_count": 3,"#, "");
    let (mut running, pump) = start_pumped(&loaded);
    running
        .set_control("tune", "degree_count", number(2.0))
        .unwrap();
    pump.render(1);
    // The document holds the scale but no count: the tweak is the
    // performer's, as any write to a key the document does not hold.
    let report = running.reload(doc(&loaded)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(read(&running, "tune", "degree_count"), number(2.0));
}

#[test]
fn a_reloaded_scale_replaying_a_degree_no_control_takes_rebuilds() {
    let held = MELODY.replace(r#""seed": 42"#, r#""seed": 42, "degree.130": 5"#);
    let (mut running, pump) = start_pumped(&held);
    let edited = held.replace("[0, 2, 4]", "[0, 7, 9]");
    let report = running.reload(doc(&edited)).expect("diff applies");
    pump.render(1);
    assert_eq!(report.swapped, ["tune"], "{report:?}");
    assert_plays_as_built(&running, &edited);
}

/// Asserts the live melody plays what a cold build of `json`'s does.
fn assert_plays_as_built(running: &RunningInvention, json: &str) {
    let document = doc(json);
    let spec = document
        .modules
        .iter()
        .find(|spec| spec.id == "tune")
        .unwrap();
    let built = crate::ModuleRegistry::default()
        .build("melody", SAMPLE_RATE, &spec.config)
        .unwrap()
        .control_surface
        .unwrap();
    let count = built.get_control("degree_count").unwrap();
    assert_eq!(read(running, "tune", "degree_count"), count);
    let ControlValue::Number(count) = count else {
        unreachable!()
    };
    for i in 0..count as usize {
        for key in [format!("degree.{i}"), format!("note_weight.{i}")] {
            assert_eq!(
                read(running, "tune", &key),
                built.get_control(&key).unwrap(),
                "{key}"
            );
        }
    }
}

#[test]
fn a_reloaded_scale_applies_before_a_degree_written_within_it() {
    let (mut running, pump) = start_pumped(MELODY);
    let edited = MELODY.replace(
        r#""degrees": "[0, 2, 4]", "degree_count": 3,"#,
        r#""degrees": "[0, 7, 9, 11]", "degree_count": 3, "degree.1": 3,"#,
    );
    let report = running.reload(doc(&edited)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(read(&running, "tune", "degree.1"), number(3.0));
    assert_eq!(read(&running, "tune", "degree.2"), number(9.0));
    // The count, unchanged, is written again after the scale.
    assert_eq!(read(&running, "tune", "degree_count"), number(3.0));
    assert_plays_as_built(&running, &edited);
}

#[test]
fn a_reloaded_scale_keeps_the_unchanged_values_written_within_it() {
    let before = MELODY.replace(
        r#""degree_count": 3,"#,
        r#""degree_count": 3, "degree.2": 1, "note_weights": "[1, 1, 1]", "note_weight.1": 4,"#,
    );
    let after = before
        .replace("[0, 2, 4]", "[0, 7, 9, 11]")
        .replace("[1, 1, 1]", "[2, 2, 2]");
    let (mut running, pump) = start_pumped(&before);
    let report = running.reload(doc(&after)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(read(&running, "tune", "degree.2"), number(1.0));
    assert_eq!(read(&running, "tune", "note_weight.1"), number(4.0));
    assert_plays_as_built(&running, &after);
}

#[test]
fn a_developments_scale_applies_before_a_degree_written_within_it() {
    // The degree's key sorts before the scale's.
    let json = r#"{
        "version": "1.0.0",
        "developments": [{ "name": "line", "definition": {
            "version": "1.0.0",
            "modules": [{ "id": "tune", "type": "melody", "config": { "seed": 1 } }],
            "connections": [],
            "outputs": [{ "name": "frequency", "from": "tune", "from_port": "frequency" }],
            "controls": [
                { "name": "a_step", "module": "tune", "control": "degree.1" },
                { "name": "scale", "module": "tune", "control": "degrees" }
            ]
        } }],
        "modules": [
            { "id": "line", "type": "line",
              "config": { "a_step": 3, "scale": "[0, 7, 9]" } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "line", "from_port": "frequency", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let (running, pump) = start_pumped(json);
    pump.render(1);
    assert_eq!(read(&running, "line", "a_step"), number(3.0));
}
