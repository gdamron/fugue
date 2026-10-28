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
use crate::{ControlSurface, ControlValue};

use super::scalar_control_value;
use crate::traits::exact_number::json_scalar;

/// `config` without `keys`, which a running module holds no state for (see
/// [`stateless_keys`]).
pub(super) fn without<'a>(config: &'a Value, keys: &[&str]) -> std::borrow::Cow<'a, Value> {
    match config.as_object() {
        Some(map) if keys.iter().any(|key| map.contains_key(*key)) => {
            let mut map = map.clone();
            keys.iter().for_each(|key| {
                map.remove(*key);
            });
            std::borrow::Cow::Owned(Value::Object(map))
        }
        _ => std::borrow::Cow::Borrowed(config),
    }
}

/// The keys of `configs` a running module holds no state for: `reload_only`
/// (see [`crate::ModuleFactory::reload_keys`]) and those `is_event` names
/// one of its event controls.
pub(super) fn stateless_keys<'a>(
    reload_only: &[&'a str],
    configs: [&'a Value; 2],
    is_event: impl Fn(&str) -> bool,
) -> Vec<&'a str> {
    let mut keys = reload_only.to_vec();
    let present = configs
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(Map::keys);
    for key in present {
        if !keys.contains(&key.as_str()) && is_event(key) {
            keys.push(key);
        }
    }
    keys
}

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
/// expressed as controls (a non-scalar value, a key the module does not
/// expose as a control, or a removed key `restore` gives no write for) and
/// the module must be swapped.
///
/// A removed key returns by the write `restore` gives it, as a fresh build
/// of `new` plays it, so deleting a setting glides back to its default as a
/// key the file never stated does (see [`implicit_restores`]).
pub(super) fn control_updates_for(
    previous: &Value,
    new: &Value,
    kind: &dyn Fn(&str) -> Option<ConfigKind>,
    mut has_control: impl FnMut(&str) -> bool,
    restore: &dyn Fn(&str) -> Option<(String, ControlValue)>,
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

    let mut updates: Vec<(String, ControlValue)> = Vec::new();
    for key in previous.keys().filter(|key| !new.contains_key(*key)) {
        if !has_control(key) {
            return None;
        }
        let (written, value) = restore(key)?;
        if !has_control(&written) {
            return None;
        }
        push_once(&mut updates, written, value);
    }
    for (key, value) in new {
        if previous
            .get(key)
            .is_some_and(|old| same_value(kind, key, old, value))
        {
            continue;
        }
        let control_value = json_scalar(value)?;
        if !has_control(key) {
            return None;
        }
        push_once(&mut updates, key.clone(), control_value);
    }
    Some(updates)
}

/// Adds a write of `key`, unless `updates` already writes it (a whole key
/// two removed positions restore by, or a removal and a change both do).
fn push_once(updates: &mut Vec<(String, ControlValue)>, key: String, value: ControlValue) {
    if !updates.iter().any(|(written, _)| *written == key) {
        updates.push((key, value));
    }
}

/// Restores of a kept module's controls `written` by authored writes its
/// stored config does not record (see
/// [`WrittenSince`](crate::invention::state::WrittenSince)): each that
/// `config`, the new one, does not state as a scalar, and no update in
/// `pending` writes, returns by the write `restore` gives it, as a fresh
/// build of `config` plays it. So reloading a file sounds as the file does,
/// the keys it leaves implicit included, without rebuilding the module.
///
/// An event is never restored: writing it would fire it. A control
/// `restore` gives no write for, or one a reload does not write, keeps what
/// was written.
pub(super) fn implicit_restores(
    written: &[String],
    config: &Value,
    pending: &[(String, ControlValue)],
    is_event: impl Fn(&str) -> bool,
    mut has_control: impl FnMut(&str) -> bool,
    restore: &dyn Fn(&str) -> Option<(String, ControlValue)>,
) -> Vec<(String, ControlValue)> {
    let states =
        |key: &str| (config.get(key)).is_some_and(|value| scalar_control_value(value).is_some());
    let mut restores = Vec::new();
    let candidates = (written.iter())
        .filter(|key| !states(key) && !is_event(key))
        .filter(|key| !pending.iter().any(|(updated, _)| updated == *key));
    for key in candidates {
        if !has_control(key) {
            continue;
        }
        let Some((key, value)) = restore(key) else {
            continue;
        };
        let pending = pending.iter().any(|(updated, _)| *updated == key);
        if !pending && has_control(&key) {
            push_once(&mut restores, key, value);
        }
    }
    restores
}

/// What control `key` of a freshly built module holds, when a write of it
/// plays that: a number outside the range its module clamps writes to (a
/// melody's count of an empty scale, zero, where writes start at one) has
/// no write that restores it, and neither has the empty name of a control
/// that loads what it names (a player built with no sample).
pub(super) fn fresh_value(surface: &dyn ControlSurface, key: &str) -> Option<ControlValue> {
    let value = surface.get_control(key).ok()?;
    let decl = surface.declaration(key).map(|declared| declared.decl);
    match (&value, decl) {
        (ControlValue::Number(number), Some(decl))
            if decl
                .clamp
                .is_some_and(|(min, max)| !(min..=max).contains(number)) =>
        {
            None
        }
        (ControlValue::String(name), Some(decl)) if decl.loads_on_write && name.is_empty() => None,
        _ => Some(value),
    }
}
