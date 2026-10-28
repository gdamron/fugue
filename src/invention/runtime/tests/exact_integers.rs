//! A whole-number control sees a write exactly, through every front door
//! that takes one: a fraction an f32 would round whole is refused by the
//! control's own rule, as text and as a JSON number, and a whole number
//! lands as it always did.

use super::*;
use crate::invention::render::RenderEngine;
use crate::rpc::{RpcCommand, StructuralEdit};
use crate::{ControlWrite, ControlWriteIntent};

/// A clamped whole-number control (a step sequencer's root note, 0 to 127),
/// which takes any whole number into its range and refuses a fraction.
const SEQUENCER: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "seq", "type": "step_sequencer", "config": { "root_note": 48 } }
    ],
    "connections": []
}"#;

/// Fractions an f32 rounds to a whole number.
const FRACTIONS: [&str; 3] = ["60.0000001", "16777214.5", "8388608.5"];

/// `text` as a client writes it: as text, and as a JSON number read off
/// the wire.
fn written(text: &str) -> [ControlValue; 2] {
    let wire = format!(r#"{{ "module_id": "seq", "key": "root_note", "value": {text} }}"#);
    let write: ControlWrite = serde_json::from_str(&wire).unwrap();
    [ControlValue::String(text.to_string()), write.value]
}

fn start() -> RunningInvention {
    let invention = Invention::from_json(SEQUENCER).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap()
}

fn authored_root_note(running: &RunningInvention) -> serde_json::Value {
    let document = running.document().unwrap();
    let module = document.modules.iter().find(|module| module.id == "seq");
    module.unwrap().config["root_note"].clone()
}

fn refused_as_fraction(error: impl std::fmt::Display, value: &ControlValue) {
    let error = error.to_string();
    assert!(
        error.contains("expects a whole number"),
        "{value:?}: {error}"
    );
}

#[test]
fn a_live_write_refuses_a_fraction_an_f32_rounds_whole() {
    let running = start();
    for text in FRACTIONS {
        for value in written(text) {
            for intent in [ControlWriteIntent::Author, ControlWriteIntent::Perform] {
                let snapshot = running.snapshot();
                let refused =
                    snapshot.set_control_with_intent("seq", "root_note", value.clone(), intent);
                refused_as_fraction(refused.unwrap_err(), &value);
            }
            let batch =
                running.set_controls(&[ControlWrite::new("seq", "root_note", value.clone())]);
            refused_as_fraction(batch.unwrap_err(), &value);
        }
    }
    assert_eq!(
        running.get_control("seq", "root_note").unwrap(),
        ControlValue::Number(48.0)
    );
    assert_eq!(authored_root_note(&running), serde_json::json!(48));

    // A whole number lands, recorded as a number.
    for value in written("61") {
        running.set_control("seq", "root_note", value).unwrap();
    }
    let landed = || running.get_control("seq", "root_note").unwrap() == ControlValue::Number(61.0);
    assert!(crate::test_support::wait_until(landed));
    let recorded = || authored_root_note(&running).as_f64() == Some(61.0);
    assert!(
        crate::test_support::wait_until(recorded),
        "{}",
        authored_root_note(&running)
    );
    running.stop();
}

#[test]
fn an_rpc_set_control_reads_its_number_exactly() {
    let wire = r#"{ "command": "set_control", "module_id": "seq", "key": "root_note", "value": 8388608.5 }"#;
    let RpcCommand::SetControl { value, .. } = serde_json::from_str(wire).unwrap() else {
        panic!("not a set_control");
    };
    assert_eq!(value, ControlValue::String("8388608.5".to_string()));
}

#[test]
fn an_offline_write_refuses_a_fraction_an_f32_rounds_whole() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(SEQUENCER).unwrap();
    for text in FRACTIONS {
        for value in written(text) {
            let refused = engine.set_control("seq", "root_note", value.clone());
            refused_as_fraction(refused.unwrap_err(), &value);
        }
    }
    assert_eq!(
        engine.get_control("seq", "root_note").unwrap(),
        ControlValue::Number(48.0)
    );
    engine
        .set_control("seq", "root_note", written("61")[1].clone())
        .unwrap();
    assert_eq!(
        engine.get_control("seq", "root_note").unwrap(),
        ControlValue::Number(61.0)
    );
}

#[test]
fn an_edit_batch_refuses_a_fraction_an_f32_rounds_whole() {
    let mut running = start();
    for text in FRACTIONS {
        for value in [format!("{text:?}"), text.to_string()] {
            let edit = format!(
                r#"{{ "op": "set_control", "module_id": "seq", "key": "root_note", "value": {value} }}"#
            );
            let edit: StructuralEdit = serde_json::from_str(&edit).unwrap();
            let refused = running.apply_edits(&[edit]).unwrap_err();
            assert!(
                refused.message.contains("expects a whole number"),
                "{value}: {refused:?}"
            );
        }
    }
    assert_eq!(authored_root_note(&running), serde_json::json!(48));

    let edit = r#"{ "op": "set_control", "module_id": "seq", "key": "root_note", "value": 61 }"#;
    running
        .apply_edits(&[serde_json::from_str(edit).unwrap()])
        .unwrap();
    assert_eq!(authored_root_note(&running).as_f64(), Some(61.0));
    running.stop();
}

#[test]
fn control_keys_a_build_applies_refuse_a_fraction_an_f32_rounds_whole() {
    let registry = crate::ModuleRegistry::default();
    let built = registry
        .build("step_sequencer", 48_000, &serde_json::json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    for text in FRACTIONS {
        let number: serde_json::Value = serde_json::from_str(text).unwrap();
        for value in [number, serde_json::json!(text)] {
            let config = serde_json::json!({ "root_note": value });
            let refused = crate::factory::apply_control_keys(surface.as_ref(), &config, |_| true);
            refused_as_fraction(refused.unwrap_err(), &value.to_string().into());
        }
    }
    let config = serde_json::json!({ "root_note": 61 });
    crate::factory::apply_control_keys(surface.as_ref(), &config, |_| true).unwrap();
    assert_eq!(
        surface.get_control("root_note").unwrap(),
        ControlValue::Number(61.0)
    );
}
