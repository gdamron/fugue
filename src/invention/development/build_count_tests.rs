//! A development instance builds its definition once, however its config
//! starts it: an event key is left out of the fold before the build rather
//! than built and built again, and a refused value costs one build more.

use serde_json::{json, Value};

use super::declared_tests::factory;
use super::starting_values::ASSEMBLED;
use crate::{ControlValue, ModuleFactory, RenderEngine};

/// How many definitions `build` builds on this thread, with its result.
fn counted<R>(build: impl FnOnce() -> R) -> (R, usize) {
    let before = ASSEMBLED.with(|count| count.get());
    let result = build();
    (result, ASSEMBLED.with(|count| count.get()) - before)
}

/// A development `name` stepping a three-cell sequencer, exposing its
/// `next_cell` event as `advance`, its playing `cell` as `at` and a vca's
/// `level` as `level`. With `inner`, it also holds an instance `inner` of
/// that development, configured to fire its own `advance`, and exposes
/// that instance's `advance` as `deep`.
fn stepping(name: &str, inner: Option<&str>) -> Value {
    let mut modules = vec![
        json!({ "id": "seq", "type": "cell_sequencer",
                "config": { "cells": [[{ "note": 0 }], [{ "note": 7 }], [{ "note": 9 }]] } }),
        json!({ "id": "amp", "type": "vca" }),
    ];
    let mut controls = vec![
        json!({ "name": "advance", "module": "seq", "control": "next_cell" }),
        json!({ "name": "at", "module": "seq", "control": "cell" }),
        json!({ "name": "level", "module": "amp", "control": "level" }),
    ];
    if let Some(inner) = inner {
        modules.push(json!({ "id": "inner", "type": inner,
            "config": { "advance": 1, "deep": 1, "level": 0.25 } }));
        controls.push(json!({ "name": "deep", "module": "inner", "control": "advance" }));
    } else {
        controls.push(json!({ "name": "deep", "module": "seq", "control": "next_cell" }));
    }
    json!({ "name": name, "definition": {
        "version": "1.0.0",
        "modules": modules,
        "connections": [{ "from": "seq", "from_port": "frequency", "to": "amp", "to_port": "audio" }],
        "outputs": [{ "name": "out", "from": "amp", "from_port": "audio" }],
        "controls": controls
    } })
}

#[test]
fn an_instance_given_an_event_builds_its_definition_once_and_fires_nothing() {
    let definition = stepping("step", None)["definition"].clone();
    let config = json!({ "advance": 1, "level": 0.5 });
    let (built, builds) = counted(|| factory("step", definition).build(48_000, &config));
    assert_eq!(builds, 1);
    let surface = built.unwrap().control_surface.unwrap();
    assert_eq!(
        surface.get_control("at").unwrap(),
        ControlValue::Number(0.0)
    );
    assert_eq!(
        surface.get_control("level").unwrap(),
        ControlValue::Number(0.5)
    );
}

#[test]
fn an_instance_given_a_refused_value_builds_its_definition_twice() {
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "s", "type": "step_sequencer" }],
        "connections": [],
        "outputs": [{ "name": "gate", "from": "s", "from_port": "gate" }],
        "controls": [{ "name": "note", "module": "s", "control": "root_note" }]
    });
    let config = json!({ "note": 60.5 });
    let (built, builds) = counted(|| factory("seq", definition).build(48_000, &config));
    assert!(built.is_ok());
    assert_eq!(builds, 2);
}

#[test]
fn nested_instances_given_events_at_every_level_build_once_each() {
    let document = json!({
        "version": "1.0.0",
        "developments": [stepping("three", None), stepping("two", Some("three")), stepping("one", Some("two"))],
        "modules": [
            { "id": "lead", "type": "one", "config": { "advance": 1, "deep": 1, "level": 0.5 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "lead", "from_port": "out", "to": "dac", "to_port": "audio" }]
    });
    let mut engine = RenderEngine::new(48_000);
    let (loaded, builds) = counted(|| engine.load_json(&document.to_string()));
    loaded.unwrap();
    assert_eq!(builds, 3, "one build per level");
    assert_eq!(
        engine.get_control("lead", "at").unwrap(),
        ControlValue::Number(0.0)
    );
}
