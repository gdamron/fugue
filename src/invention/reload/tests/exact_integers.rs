//! A reload writes a whole-number control exactly: a fraction an f32 would
//! round whole is refused, as a JSON number and as text, and a whole number
//! lands as it always did.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::start_manual;

/// A dial whose `span` (a clamped whole number, 1 to 64) its config does
/// not read, so a reload changing it writes the control.
fn dial(span: serde_json::Value) -> Invention {
    let document = json!({
        "version": "1.0.0",
        "modules": [{ "id": "d", "type": "dial", "config": { "span": span } }],
        "connections": []
    });
    doc(&document.to_string())
}

#[test]
fn a_reload_refuses_a_fraction_an_f32_rounds_whole() {
    let mut registry = crate::ModuleRegistry::default();
    registry.register(crate::test_support::dial::DialFactory);
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(dial(json!(4)))
        .unwrap();
    let (mut running, pump) = start_manual(runtime);
    for text in ["60.0000001", "16777214.5", "8388608.5"] {
        let number: serde_json::Value = serde_json::from_str(text).unwrap();
        for value in [number, json!(text)] {
            let refused = running.reload(dial(value.clone()));
            let error = refused.expect_err(&format!("{value} reloaded")).to_string();
            assert!(error.contains("expects a whole number"), "{value}: {error}");
        }
    }
    pump.render(1);
    let span = running.get_control("d", "span").unwrap();
    assert_eq!(span, ControlValue::Number(4.0));

    let report = running.reload(dial(json!(61))).unwrap();
    assert!(report.swapped.is_empty(), "{report:?}");
    pump.render(1);
    let span = running.get_control("d", "span").unwrap();
    assert_eq!(span, ControlValue::Number(61.0));
}
