//! Event controls hold no state, so the document never authors one: an
//! event written as authored, live or in an edit batch, fires once and
//! leaves no key, and a config key naming an event neither fires nor
//! rebuilds its module on reload. A clock's reset on reload is its own
//! write (see `clock.rs`).

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::rpc::StructuralEdit;

/// A three-cell sequencer `seq` (playing cell 0 until its `next_cell`
/// event fires), with `extra` appended to its config.
fn sequencer(extra: &str) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "modules": [
            {{ "id": "seq", "type": "cell_sequencer",
               "config": {{ "cells": [[{{ "note": 0 }}], [{{ "note": 7 }}], [{{ "note": 9 }}]]{extra} }} }},
            {{ "id": "dac", "type": "dac" }}
        ],
        "connections": [
            {{ "from": "seq", "from_port": "frequency", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

fn started(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(2);
    (running, pump)
}

fn cell(running: &RunningInvention, id: &str) -> ControlValue {
    running.get_control(id, "cell").unwrap()
}

fn next_cell(id: &str) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: id.into(),
        key: "next_cell".into(),
        value: ControlValue::Number(1.0),
    }
}

/// The retained document as saving writes it, and whether it names
/// `next_cell` anywhere.
fn saved(running: &RunningInvention) -> (Invention, bool) {
    let json = running.document().unwrap().to_json().unwrap();
    let names_event = json.contains("next_cell");
    (Invention::from_json(&json).unwrap(), names_event)
}

/// Reloads `invention` and asserts nothing fired, was written or rebuilt.
fn reload_quietly(running: &mut RunningInvention, pump: &Pump, invention: Invention) {
    let before = cell(running, "seq");
    let report = running.reload(invention).unwrap();
    pump.render(2);
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(cell(running, "seq"), before, "no event fired");
}

#[test]
fn an_authored_event_write_fires_once_and_saves_no_key() {
    let (mut running, pump) = started(&sequencer(""));
    // The wire's default intent is authoring.
    running
        .set_control("seq", "next_cell", ControlValue::Number(1.0))
        .unwrap();
    pump.render(2);
    assert_eq!(cell(&running, "seq"), ControlValue::Number(1.0));

    let (document, names_event) = saved(&running);
    assert!(!names_event, "the document holds no event key");
    reload_quietly(&mut running, &pump, document);
}

#[test]
fn an_edit_batch_event_fires_once_and_saves_no_key() {
    let (mut running, pump) = started(&sequencer(""));
    let report = running.apply_edits(&[next_cell("seq")]).unwrap();
    pump.render(2);
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(cell(&running, "seq"), ControlValue::Number(1.0));

    let (document, names_event) = saved(&running);
    assert!(!names_event, "the document holds no event key");
    reload_quietly(&mut running, &pump, document);
}

#[test]
fn an_event_written_to_a_module_the_batch_adds_fires_as_it_runs() {
    let (mut running, pump) = started(&sequencer(""));
    let add = StructuralEdit::AddModule {
        id: "seq2".into(),
        module_type: "cell_sequencer".into(),
        config: serde_json::json!({ "cells": [[{ "note": 0 }], [{ "note": 7 }], [{ "note": 9 }]] }),
    };
    let report = running.apply_edits(&[add, next_cell("seq2")]).unwrap();
    pump.render(2);
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(cell(&running, "seq2"), ControlValue::Number(1.0));
    assert!(!saved(&running).1, "the document holds no event key");
}

#[test]
fn a_config_key_naming_an_event_neither_fires_nor_rebuilds_on_reload() {
    let (mut running, pump) = started(&sequencer(""));
    running
        .set_control("seq", "next_cell", ControlValue::Number(1.0))
        .unwrap();
    pump.render(2);
    // Written in by hand, then changed, then removed: the sequencer keeps
    // its instance, and the cell it plays, throughout.
    for extra in [r#", "next_cell": 1"#, r#", "next_cell": 0"#, ""] {
        reload_quietly(&mut running, &pump, doc(&sequencer(extra)));
        assert_eq!(cell(&running, "seq"), ControlValue::Number(1.0), "{extra}");
    }
}

#[test]
fn an_event_on_a_module_replacing_one_written_earlier_is_not_authored() {
    let json = r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "x", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "x", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    }"#;
    let (mut running, pump) = started(json);
    // `x` is written as an oscillator, then replaced by a sequencer, whose
    // event the batch writes.
    let tune = StructuralEdit::SetControl {
        module_id: "x".into(),
        key: "frequency".into(),
        value: ControlValue::Number(330.0),
    };
    let add = StructuralEdit::AddModule {
        id: "x".into(),
        module_type: "cell_sequencer".into(),
        config: serde_json::json!({ "cells": [[{ "note": 0 }], [{ "note": 7 }]] }),
    };
    let remove = StructuralEdit::RemoveModule { id: "x".into() };
    let connect = StructuralEdit::Connect {
        from: "x".into(),
        from_port: "frequency".into(),
        to: "dac".into(),
        to_port: "audio".into(),
    };
    let edits = [tune, remove, add, connect, next_cell("x")];
    let report = running.apply_edits(&edits).unwrap();
    pump.render(2);
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert!(!saved(&running).1, "the document holds no event key");
}
