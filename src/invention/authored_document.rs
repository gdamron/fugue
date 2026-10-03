//! Edits to an authored invention document.
//!
//! The single implementation of how a module or control change is written
//! into an authored document. Runtime mutations use it to keep the retained
//! document in step with the live graph (see [`RuntimeState`]); an atomic
//! edit batch uses it to build its candidate document. Sharing it means a
//! document saved after either path is identical.
//!
//! [`RuntimeState`]: super::state::RuntimeState

use crate::invention::format::{Invention, ModuleSpec};
use crate::ControlValue;

/// Records a module added, replaced, or swapped: updates the spec with the
/// same id in place, or appends a new one.
pub(crate) fn upsert_module(
    document: &mut Invention,
    id: &str,
    module_type: &str,
    config: &serde_json::Value,
) {
    match document.modules.iter_mut().find(|spec| spec.id == id) {
        Some(spec) => {
            spec.module_type = module_type.to_string();
            spec.config = config.clone();
        }
        None => document.modules.push(ModuleSpec {
            id: id.to_string(),
            module_type: module_type.to_string(),
            config: config.clone(),
        }),
    }
}

/// Removes a module's spec. Connections are left to the caller.
pub(crate) fn remove_module(document: &mut Invention, id: &str) {
    document.modules.retain(|spec| spec.id != id);
}

/// Removes `keys` from a module's config: controls the module no longer
/// lists (a melody's `degree.6` once its count shrank to three), so a cold
/// rebuild does not restore a value the live module dropped.
pub(crate) fn forget_controls(document: &mut Invention, id: &str, keys: &[String]) {
    let Some(spec) = document.modules.iter_mut().find(|spec| spec.id == id) else {
        return;
    };
    if let Some(config) = spec.config.as_object_mut() {
        for key in keys {
            config.remove(key);
        }
    }
}

/// Writes a control value into a module's config so the document reproduces
/// it on a cold rebuild. Does nothing when no module has that id.
pub(crate) fn write_control(document: &mut Invention, id: &str, key: &str, value: &ControlValue) {
    let Some(spec) = document.modules.iter_mut().find(|spec| spec.id == id) else {
        return;
    };
    let value = match value {
        // Integral values stay JSON integers (a step count written as
        // 16.0 would spuriously differ from the authored 16 on every
        // reload diff); fractional values widen through the shortest
        // decimal form of the f32 (its Display output) so 0.7f32 lands
        // in the document as 0.7, not 0.699999988079071.
        ControlValue::Number(number) if number.fract() == 0.0 && number.abs() < 2e15 => {
            serde_json::json!(*number as i64)
        }
        ControlValue::Number(number) => number
            .to_string()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ControlValue::Bool(flag) => serde_json::json!(flag),
        ControlValue::String(text) => serde_json::json!(text),
    };
    if !spec.config.is_object() {
        spec.config = serde_json::Value::Object(serde_json::Map::new());
    }
    spec.config
        .as_object_mut()
        .expect("config was just made an object")
        .insert(key.to_string(), value);
}
