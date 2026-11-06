//! A declared range holds automation's writes too, and a development
//! nesting another narrows the nested one's range in turn.

use serde_json::json;

use super::*;
use crate::test_support::dial::DialFactory;

fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    registry
}

fn dial_development(range: serde_json::Value) -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": [{ "name": "level", "module": "a", "control": "level", "range": range }]
    }))
    .unwrap()
}

#[test]
fn automation_on_a_ranged_control_holds_its_writes_within_the_range() {
    let definition = dial_development(json!({ "min": 0.25, "max": 0.5 }));
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry())
        .build(definition.clone())
        .unwrap();
    let inner = runtime.control_surfaces.lock().unwrap()["a"].clone();
    let (_module, surface) = DevelopmentModule::new("one", runtime, &definition).unwrap();
    let automation = surface.automation("level").unwrap();
    automation.write_number(0.9);
    assert_eq!(automation.current(), Some(0.5));
    // The alias's own slot holds the clamped write, as its module takes it.
    assert_eq!(inner.automation("level").unwrap().current(), Some(0.5));
    automation.write_number(0.0);
    assert_eq!(automation.current(), Some(0.25));
}

#[test]
fn a_nesting_development_narrows_a_nested_range_and_may_not_widen_it() {
    let nested = |range: serde_json::Value| {
        json!({
            "version": "1.0.0",
            "developments": [{ "name": "inner", "definition": dial_development(json!({ "min": 0.2, "max": 0.8 })) }],
            "modules": [{ "id": "i", "type": "inner", "config": { "level": 0.5 } }],
            "connections": [],
            "outputs": [{ "name": "a", "from": "i", "from_port": "a" }],
            "controls": [{ "name": "level", "module": "i", "control": "level", "range": range }]
        })
    };
    let build = |range| {
        let document = json!({
            "version": "1.0.0",
            "developments": [{ "name": "outer", "definition": nested(range) }],
            "modules": [{ "id": "o", "type": "outer" }],
            "connections": []
        });
        let invention: Invention = serde_json::from_value(document).unwrap();
        InventionBuilder::with_registry(48_000, registry()).build(invention)
    };
    let (runtime, _) = build(json!({ "min": 0.3, "max": 0.6 })).unwrap();
    let outer = runtime.control_surfaces.lock().unwrap()["o"].clone();
    let listed = outer.controls();
    let level = listed.iter().find(|meta| meta.key == "level").unwrap();
    assert!(matches!(
        level.kind,
        crate::ControlKind::Number { min, max, .. } if min == 0.3 && max == 0.6
    ));
    let error = build(json!({ "min": 0.1, "max": 0.6 })).err().unwrap();
    let error = error.to_string();
    assert!(error.contains("outer") && error.contains("0.1") && error.contains("0.2"));
}

/// A main graph holding one instance of `definition`, as `kit`, with
/// `config`.
fn holding(definition: &Invention, config: serde_json::Value) -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "kit", "definition": definition }],
        "modules": [{ "id": "k", "type": "kit", "config": config }],
        "connections": []
    }))
    .unwrap()
}

#[test]
fn a_definition_starting_a_ranged_control_outside_its_range_starts_within_it() {
    let mut definition = dial_development(json!({ "min": 0.5, "max": 0.75 }));
    definition.modules[0].config = json!({ "level": 0.25 });
    let build = |config| {
        let invention = holding(&definition, config);
        InventionBuilder::with_registry(48_000, registry()).build(invention)
    };
    let (runtime, _) = build(json!({})).unwrap();
    let kit = runtime.control_surfaces.lock().unwrap()["k"].clone();
    assert_eq!(kit.get_control("level"), Ok(ControlValue::Number(0.5)));
    let listed = kit.controls();
    assert_eq!(listed[0].default, ControlValue::Number(0.5));
    // A configured value still wins over the definition's start.
    let (runtime, _) = build(json!({ "level": 0.7 })).unwrap();
    let kit = runtime.control_surfaces.lock().unwrap()["k"].clone();
    assert_eq!(kit.get_control("level"), Ok(ControlValue::Number(0.7)));
}

#[test]
fn a_range_on_a_control_that_is_not_a_number_is_refused() {
    let definition: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": [{ "name": "steps", "module": "a", "control": "steps",
                       "range": { "min": 2, "max": 4 } }]
    }))
    .unwrap();
    let error = InventionBuilder::with_registry(48_000, registry())
        .build(holding(&definition, json!({})))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("'kit'") && error.contains("'steps'") && error.contains("a.steps"));
}

#[test]
fn a_fan_out_whose_later_alias_starts_outside_its_range_starts_every_alias_within() {
    let definition: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [
            { "id": "a", "type": "lfo", "config": { "rate": 2.0 } },
            { "id": "b", "type": "lfo", "config": { "rate": 50.0 } }
        ],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "bipolar" }],
        "controls": [
            { "name": "rate", "module": "a", "control": "rate",
              "range": { "min": 0.05, "max": 10.0 } },
            { "name": "rate", "module": "b", "control": "rate" }
        ]
    }))
    .unwrap();
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry())
        .build(holding(&definition, json!({})))
        .unwrap();
    let module = &runtime.modules["k"];
    let mut values = Vec::new();
    module
        .module()
        .for_each_alias(ControlIndex(0), &mut |table, cells, index| {
            values.push(table.value(index, cells.load(index).unwrap()));
        });
    assert_eq!(
        values,
        vec![
            Some(ControlValue::Number(2.0)),
            Some(ControlValue::Number(2.0))
        ]
    );
    // A present null is still refused, as it is for every other control.
    let refused = InventionBuilder::with_registry(48_000, registry())
        .build(holding(&definition, json!({ "rate": null })));
    assert!(refused.is_err());
}
