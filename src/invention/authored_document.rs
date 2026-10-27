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
use crate::module_config::{names, replaced_by, ConfigKey};
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

/// Writes a control value into a module's config so the document reproduces
/// it on a cold rebuild, dropping the keys the write replaces (see
/// [`forget_replaced`]) among those `keys_of` its type declares. Does
/// nothing when no module has that id.
pub(crate) fn write_control(
    document: &mut Invention,
    id: &str,
    key: &str,
    value: &ControlValue,
    keys_of: impl FnOnce(&str) -> &'static [ConfigKey],
) {
    if let Some(spec) = document.modules.iter_mut().find(|spec| spec.id == id) {
        forget_replaced(
            &mut spec.config,
            replaced_by(keys_of(&spec.module_type), key),
        );
        write_config_control(&mut spec.config, key, value);
    }
}

/// Drops from a config the `replaced` keys, with their positions, that a
/// write rewrites wholesale (see [`ConfigKey::replacing`]), so the config
/// never rebuilds what the write discarded. Returns whether it replaces
/// any.
pub(crate) fn forget_replaced(config: &mut serde_json::Value, replaced: &[&str]) -> bool {
    if replaced.is_empty() {
        return false;
    }
    if let Some(config) = config.as_object_mut() {
        config.retain(|name, _| !replaced.iter().any(|gone| names(gone, name)));
    }
    true
}

/// The keys of `config` among the `replaced` (see [`forget_replaced`]), in
/// the config's order.
pub(crate) fn replaced_in<'a>(
    config: &'a serde_json::Value,
    replaced: &'a [&'a str],
) -> impl Iterator<Item = (&'a String, &'a serde_json::Value)> {
    (config.as_object().into_iter().flatten())
        .filter(move |(name, _)| replaced.iter().any(|gone| names(gone, name)))
}

/// Writes a control value into one config under the control's key, making
/// the config an object first when it is not one.
///
/// A sample's `asset.N` is the exception: it lands in the `asset` of the
/// entry it names (see [`entry_asset_mut`]), and a top-level `asset.N` an
/// older save recorded is dropped, as the entry now holds the write.
pub(crate) fn write_config_control(
    config: &mut serde_json::Value,
    key: &str,
    value: &ControlValue,
) {
    if write_entry_asset(config, key, &control_json(value)) {
        return;
    }
    if !config.is_object() {
        *config = serde_json::Value::Object(serde_json::Map::new());
    }
    config
        .as_object_mut()
        .expect("config was just made an object")
        .insert(key.to_string(), control_json(value));
}

/// Writes `asset` into the entry an `asset.N` control names (see
/// [`entry_asset_mut`]) and drops a top-level `asset.N` an older save
/// recorded, as the entry now holds the write. Returns false, changing
/// nothing, when `key` names no such entry.
pub(crate) fn write_entry_asset(
    config: &mut serde_json::Value,
    key: &str,
    asset: &serde_json::Value,
) -> bool {
    let Some(slot) = entry_asset_mut(config, key) else {
        return false;
    };
    *slot = asset.clone();
    if let Some(object) = config.as_object_mut() {
        object.remove(key);
    }
    true
}

/// What `document` holds for control `key` of module `id`, as authored:
/// under the key in its config or, for a sample's `asset.N`, as the `asset`
/// of the entry it names (see [`entry_asset_mut`]). `None` when it holds
/// nothing there.
pub(crate) fn held_control(document: &Invention, id: &str, key: &str) -> Option<serde_json::Value> {
    let config = document
        .modules
        .iter()
        .find(|spec| spec.id == id)?
        .config
        .as_object()?;
    match asset_entry(config, key) {
        Some((list, index)) => config[list][index].get("asset").cloned(),
        None => config.get(key).cloned(),
    }
}

/// Puts `held` back where [`held_control`] read it, or removes what is
/// there when `held` is `None`. Does nothing when no module has `id`.
pub(crate) fn restore_control(
    document: &mut Invention,
    id: &str,
    key: &str,
    held: Option<serde_json::Value>,
) {
    let Some(spec) = document.modules.iter_mut().find(|spec| spec.id == id) else {
        return;
    };
    if !spec.config.is_object() {
        if held.is_none() {
            return;
        }
        spec.config = serde_json::Value::Object(serde_json::Map::new());
    }
    let config = spec.config.as_object_mut().expect("config is an object");
    let (fields, field) = match asset_entry(config, key) {
        Some((list, index)) => {
            let entry = config[list][index].as_object_mut();
            (entry.expect("the entry is an object"), "asset")
        }
        None => (config, key),
    };
    match held {
        Some(value) => fields.insert(field.to_string(), value),
        None => fields.remove(field),
    };
}

/// The list and index of the sample entry an `asset.N` control names: an
/// object at `N` of a `samples` or `zones` array, not a `$asset` reference.
fn asset_entry(
    config: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<(&'static str, usize)> {
    let index: usize = key.strip_prefix("asset.")?.parse().ok()?;
    let list = ["samples", "zones"].into_iter().find(|list| {
        config
            .get(*list)
            .and_then(|entries| entries.get(index))
            .and_then(serde_json::Value::as_object)
            .is_some_and(|entry| !entry.contains_key("$asset"))
    })?;
    Some((list, index))
}

/// The `asset` of the entry an `asset.N` control names, when `config` lists
/// its samples in a `samples` or `zones` array holding an object at `N`.
///
/// A sample's asset is authored in its entry, where the build loads it and
/// the audio-asset scan, packaging and the read cap find it (see
/// `audio_assets::module_asset_values`), so a write to it is recorded there
/// too. Inserts the entry's `asset` when it has none.
///
/// An entry that is a `$asset` reference (its fields read from a JSON
/// asset) names no such slot: it may hold only the reference, so the write
/// is recorded under the top-level key instead.
pub(crate) fn entry_asset_mut<'a>(
    config: &'a mut serde_json::Value,
    key: &str,
) -> Option<&'a mut serde_json::Value> {
    let object = config.as_object_mut()?;
    let (list, index) = asset_entry(object, key)?;
    let entry = object.get_mut(list)?.get_mut(index)?.as_object_mut()?;
    Some(entry.entry("asset").or_insert(serde_json::Value::Null))
}

/// A control value as config JSON.
///
/// The single conversion of a control value to config JSON: the retained
/// document and the runtime's stored module configs both record an authored
/// write through it, so a reload of a document saved after the write diffs
/// equal to the stored config.
pub(crate) fn control_json(value: &ControlValue) -> serde_json::Value {
    match value {
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
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn document(config: serde_json::Value) -> Invention {
        let json = json!({ "version": "1.0.0", "modules": [{ "id": "m", "type": "t", "config": config }], "connections": [] });
        Invention::from_json(&json.to_string()).unwrap()
    }

    #[test]
    fn a_held_value_of_any_shape_is_put_back_as_it_was() {
        let before = document(json!({ "schedule": [{ "at": 0 }], "frequency": null }));
        let mut after = document(json!({ "schedule": "[]", "frequency": 220 }));
        for key in ["schedule", "frequency"] {
            restore_control(&mut after, "m", key, held_control(&before, "m", key));
        }
        assert_eq!(after, before);
    }

    #[test]
    fn a_control_held_nowhere_is_removed() {
        let before = document(json!({}));
        let mut after = document(json!({ "level": 0.75 }));
        restore_control(
            &mut after,
            "m",
            "level",
            held_control(&before, "m", "level"),
        );
        assert_eq!(after, before);
    }

    #[test]
    fn a_sample_asset_is_put_back_in_the_entry_it_names() {
        let before = document(json!({ "samples": [{ "asset": "a.wav" }, { "note": 60 }] }));
        let mut after = document(
            json!({ "samples": [{ "asset": "b.wav" }, { "note": 60, "asset": "c.wav" }] }),
        );
        for key in ["asset.0", "asset.1"] {
            restore_control(&mut after, "m", key, held_control(&before, "m", key));
        }
        assert_eq!(after, before);
    }
}
