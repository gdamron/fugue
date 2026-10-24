//! A development restoring a saved event: it fires once, as the
//! development first runs, and a reload keeping the development fires it
//! no more.

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
fn a_saved_event_fires_once_as_its_development_runs_and_not_again_on_reload() {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(sequence(r#"{ "advance": 1 }"#, true))
        .unwrap();
    let (mut running, pump) = start_manual(runtime);
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(1.0), "fired once");

    // The development is kept: its saved event is not written again.
    running
        .reload(sequence(r#"{ "advance": 1 }"#, false))
        .unwrap();
    pump.render(64);
    assert_eq!(at(&running), ControlValue::Number(1.0), "not again");

    // Nor is a saved `false` an event to fire.
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(sequence(r#"{ "advance": 0 }"#, true))
        .unwrap();
    let (_running, pump) = start_manual(runtime);
    pump.render(64);
    assert_eq!(at(&_running), ControlValue::Number(0.0));
}
