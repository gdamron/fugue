//! Comparing a module's stored config with the document's, key by key.
//!
//! JSON alone would count `440` and `440.0` as a change, so a reload would
//! write a control that already holds the value, overwriting anything
//! performed since. A key the module type declares
//! ([`ModuleFactory::config_keys`](crate::ModuleFactory::config_keys)), or
//! a declared control of the running module, compares by the value its
//! reader reads. Any other value compares as JSON: an undeclared key may be
//! read some other way (a `serde` struct that takes `72` but not `72.0`),
//! and comparing it by value could keep a module the file would not build
//! the same way.

use serde_json::{Map, Value};

use crate::module_config::ConfigKind;
use crate::ControlValue;

use super::scalar_control_value;

/// Whether the two configs build the same module. A null config and an
/// empty object are equivalent: omitting `config` parses as `null`, while
/// an explicit `{}` is an empty object.
pub(super) fn configs_equal(
    previous: &Value,
    new: &Value,
    kind: &dyn Fn(&str) -> Option<ConfigKind>,
) -> bool {
    if previous == new || (is_empty_config(previous) && is_empty_config(new)) {
        return true;
    }
    match (previous.as_object(), new.as_object()) {
        (Some(previous), Some(new)) => {
            previous.len() == new.len()
                && new.iter().all(|(key, value)| {
                    previous
                        .get(key)
                        .is_some_and(|old| same_value(kind, key, old, value))
                })
        }
        _ => false,
    }
}

fn is_empty_config(value: &Value) -> bool {
    value.is_null() || value.as_object().is_some_and(|map| map.is_empty())
}

/// Whether `key` holds the same value in both configs, read as `kind`
/// says it is read.
fn same_value(
    kind: &dyn Fn(&str) -> Option<ConfigKind>,
    key: &str,
    previous: &Value,
    new: &Value,
) -> bool {
    previous == new || kind(key).is_some_and(|kind| kind.same_value(previous, new))
}

/// Maps a config delta to control updates, or `None` when the delta cannot be
/// expressed as controls (a removed key, a non-scalar value, or a key the
/// module does not expose as a control) and the module must be swapped.
pub(super) fn control_updates_for(
    previous: &Value,
    new: &Value,
    kind: &dyn Fn(&str) -> Option<ConfigKind>,
    mut has_control: impl FnMut(&str) -> bool,
) -> Option<Vec<(String, ControlValue)>> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
    let previous = match previous {
        Value::Null => &*EMPTY,
        other => other.as_object()?,
    };
    let new = match new {
        Value::Null => &*EMPTY,
        other => other.as_object()?,
    };

    // A key that disappeared means "revert to the built-in default", which
    // only a rebuild of the module can express.
    if previous.keys().any(|key| !new.contains_key(key)) {
        return None;
    }

    let mut updates = Vec::new();
    for (key, value) in new {
        if previous
            .get(key)
            .is_some_and(|old| same_value(kind, key, old, value))
        {
            continue;
        }
        let control_value = scalar_control_value(value)?;
        if !has_control(key) {
            return None;
        }
        updates.push((key.clone(), control_value));
    }
    Some(updates)
}
