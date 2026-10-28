//! A development's config writes a whole-number control exactly: a
//! fraction an f32 would round whole is refused, as a JSON number and as
//! text, and a whole number builds as it always did.

use serde_json::json;

use super::declared_tests::factory;
use crate::ModuleFactory;

/// A development exposing a step sequencer's clamped root note (0 to 127).
fn sequencer() -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "modules": [{ "id": "s", "type": "step_sequencer" }],
        "connections": [],
        "outputs": [{ "name": "gate", "from": "s", "from_port": "gate" }],
        "controls": [{ "name": "note", "module": "s", "control": "root_note" }]
    })
}

#[test]
fn a_development_config_refuses_a_fraction_an_f32_rounds_whole() {
    for text in ["60.0000001", "16777214.5", "8388608.5"] {
        let number: serde_json::Value = serde_json::from_str(text).unwrap();
        for value in [number, json!(text)] {
            let built = factory("seq", sequencer()).build(48_000, &json!({ "note": value }));
            let error = built
                .err()
                .unwrap_or_else(|| panic!("{value} built"))
                .to_string();
            assert!(error.contains("expects a whole number"), "{value}: {error}");
        }
    }
    // A number control, here one its module reads itself, still takes it
    // as the f32 it rounds to.
    let mut host = sequencer();
    host["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "h", "type": "host_dial" }));
    host["controls"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "rate", "module": "h", "control": "rate" }));
    let built = factory("seq", host)
        .build(48_000, &json!({ "rate": 60.0000001 }))
        .unwrap();
    let rate = built.control_surface.unwrap().get_control("rate").unwrap();
    assert_eq!(rate, crate::ControlValue::Number(60.0));

    let built = factory("seq", sequencer())
        .build(48_000, &json!({ "note": 61 }))
        .unwrap();
    let note = built.control_surface.unwrap().get_control("note").unwrap();
    assert_eq!(note, crate::ControlValue::Number(61.0));
}
