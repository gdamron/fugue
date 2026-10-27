//! Every built-in module type with controls declares them: each write is
//! a request the audio thread applies.

use crate::module_config::tests::registry::{base_config, UNBUILDABLE};
use crate::ControlValue;

/// Every built-in type builds (from the registry harness's config) unless
/// it cannot be built in a test at all, and every one with controls
/// declares them.
#[test]
fn every_module_type_with_controls_declares_them() {
    let registry = crate::ModuleRegistry::default();
    let mut checked = Vec::new();
    for type_id in registry.types() {
        if registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let mut config = base_config(type_id);
        if type_id == "sample_slicer" {
            // Its controls exist only in elastic mode.
            config["mode"] = serde_json::json!("elastic");
        }
        let built = registry.build(type_id, 48_000, &config);
        let result = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
        if result.control_surface.is_none() {
            continue;
        }
        let declared = result.module.module().declared().is_some();
        assert!(declared, "'{type_id}' does not declare its controls");
        checked.push(type_id);
    }
    for expected in ["oscillator", "vca", "sample_instrument", "mixer"] {
        assert!(
            checked.contains(&expected),
            "'{expected}' not checked: {checked:?}"
        );
    }
}

/// A write a surface takes outside a request (an undeclared control's,
/// on the control thread) never reaches a declared control: that is the
/// request's to apply, on the audio thread. Every built type is checked,
/// each declared control holding a number or a boolean written a
/// different value.
#[test]
fn an_undeclared_write_leaves_every_declared_control_as_it_was() {
    let registry = crate::ModuleRegistry::default();
    let mut checked = 0;
    for type_id in registry.types() {
        if registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let built = registry.build(type_id, 48_000, &base_config(type_id));
        let built = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
        let Some(surface) = built.control_surface else {
            continue;
        };
        for meta in surface.controls() {
            let key = meta.key.as_str();
            let Ok(before) = surface.get_control(key) else {
                continue;
            };
            let other = match before {
                ControlValue::Number(value) => ControlValue::Number(value + 1.0),
                ControlValue::Bool(value) => ControlValue::Bool(!value),
                _ => continue,
            };
            if !surface.declares(key) {
                continue;
            }
            let _ = surface.set_undeclared(key, other);
            let after = surface.get_control(key);
            assert_eq!(after.as_ref(), Ok(&before), "'{type_id}.{key}'");
            checked += 1;
        }
    }
    assert!(checked > 0, "no declared control was checked");
}
