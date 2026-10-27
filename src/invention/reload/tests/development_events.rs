//! A development's saved event key never fires, as a plain module's never
//! does: not as the development first runs, nor as a reload keeps it.

use super::*;
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};

/// A development `seq` stepping a two-cell sequencer, exposing its
/// `next_cell` as `advance` and its playing `cell` as `at`, its instance
/// configured by `config`; and a dac whose `soft_clip` is `soft_clip`.
fn sequence(config: &str, soft_clip: bool) -> Invention {
    doc(&format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{ "name": "seq", "definition": {{
            "version": "1.0.0",
            "modules": [
                {{ "id": "clk", "type": "clock", "config": {{ "bpm": 6000 }} }},
                {{ "id": "a", "type": "cell_sequencer",
                   "config": {{ "cells": [[{{ "note": 0 }}], [{{ "note": 7 }}]] }} }}
            ],
            "connections": [
                {{ "from": "clk", "from_port": "beat", "to": "a", "to_port": "clock" }}
            ],
            "outputs": [{{ "name": "out", "from": "a", "from_port": "frequency" }}],
            "controls": [
                {{ "name": "advance", "module": "a", "control": "next_cell" }},
                {{ "name": "at", "module": "a", "control": "cell" }}
            ]
        }} }}],
        "modules": [
            {{ "id": "seq", "type": "seq", "config": {config} }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": {soft_clip} }} }}
        ],
        "connections": [
            {{ "from": "seq", "from_port": "out", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    ))
}

fn at(running: &RunningInvention) -> ControlValue {
    running.get_control("seq", "at").unwrap()
}

#[test]
fn a_saved_event_fires_neither_as_its_development_runs_nor_on_reload() {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(sequence(r#"{ "advance": 1 }"#, true))
        .unwrap();
    let (mut running, pump) = start_manual(runtime);
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(0.0), "not fired");

    // The development is kept: its saved event is not written either.
    running
        .reload(sequence(r#"{ "advance": 1 }"#, false))
        .unwrap();
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(0.0), "nor on reload");

    // Stepped by hand, it still fires, and a reload keeps its place.
    running
        .set_control("seq", "advance", ControlValue::Number(1.0))
        .unwrap();
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(1.0));
    running
        .reload(sequence(r#"{ "advance": 1 }"#, true))
        .unwrap();
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(1.0), "kept its place");
}

#[test]
fn a_plain_modules_saved_event_does_not_fire_on_a_cold_load_either() {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(doc(r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "seq", "type": "cell_sequencer",
                  "config": { "cells": [[{ "note": 0 }], [{ "note": 7 }]], "next_cell": 1 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "seq", "from_port": "frequency", "to": "dac", "to_port": "audio" }
            ]
        }"#))
        .unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(64);
    assert_eq!(
        running.get_control("seq", "cell").unwrap(),
        ControlValue::Number(0.0)
    );
}
