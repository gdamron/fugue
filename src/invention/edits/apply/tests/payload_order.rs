//! A payload rewriting a scalar (a tape's notes carrying their gain) and
//! writes to that scalar, bound to a module a batch adds, land in the order
//! made, each as written.

use serde_json::json;

use super::{add, number, set, start_with, BASE};
use crate::test_support::tape::TapeFactory;
use crate::{ControlValue, ModuleRegistry};

fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::default();
    registry.register(TapeFactory);
    registry
}

/// Notes of `[2]` carrying `gain`.
fn carrying(gain: f32) -> ControlValue {
    ControlValue::String(format!(r#"{{"notes": [2], "gain": {gain}}}"#))
}

#[test]
fn an_added_modules_writes_bind_in_the_order_made_each_as_written() {
    let (mut running, pump) = start_with(registry(), BASE);
    running
        .apply_edits(&[
            add("after", "tape", json!({ "notes": [1] })),
            set("after", "gain", number(0.5)),
            set("after", "notes", carrying(0.25)),
            set("after", "gain", number(0.75)),
            add("before", "tape", json!({ "notes": [1] })),
            set("before", "gain", number(0.5)),
            set("before", "notes", carrying(0.25)),
        ])
        .expect("the batch commits");
    pump.render(1);
    assert_eq!(running.get_control("after", "gain").unwrap(), number(0.75));
    assert_eq!(running.get_control("before", "gain").unwrap(), number(0.25));
}
