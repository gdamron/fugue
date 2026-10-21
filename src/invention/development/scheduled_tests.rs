//! A development's legacy keys take a scheduler's writes through every
//! alias's own scheduled setter: allocation- and free-free, and refused
//! without formatting a key it has not got.

use serde_json::json;

use crate::alloc_counter::allocator_events;
use crate::control_request::{LegacyRefusal, TableValue};
use crate::{ControlValue, ModuleFactory};

#[test]
fn a_scheduled_write_fans_out_to_every_legacy_alias_without_allocating() {
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "lfo" }, { "id": "b", "type": "lfo" }],
        "connections": [],
        "controls": [
            { "name": "rate", "module": "a", "control": "rate" },
            { "name": "rate", "module": "b", "control": "rate" },
            { "name": "b_rate", "module": "b", "control": "rate" }
        ]
    });
    let built = super::declared_tests::factory("legacy", definition)
        .build(48_000, &json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    let (written, allocs, frees) = allocator_events(|| {
        let written = surface.set_scheduled("rate", TableValue::Number(3.0));
        let missing = surface.set_scheduled("nope", TableValue::Number(3.0));
        let read = (
            surface.get_scheduled("b_rate"),
            surface.get_scheduled("nope"),
        );
        (written, missing, read)
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(written.0, Ok(()));
    assert_eq!(written.1, Err(LegacyRefusal::Unsupported));
    assert_eq!(written.2, (Ok(3.0), Err(LegacyRefusal::Unsupported)));
    assert_eq!(surface.get_control("rate"), Ok(ControlValue::Number(3.0)));
    // A value of the other kind is refused by every alias.
    let wrong = surface.set_scheduled("rate", TableValue::Bool(true));
    assert_eq!(wrong, Err(LegacyRefusal::Unsupported));
}
