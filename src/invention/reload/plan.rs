//! Planning a reload: the diff between the new document and the running
//! topology, as the runtime mutations that apply it.

use indexmap::IndexMap;
use std::collections::HashSet;

use crate::module_config::{names, replaced_by, ConfigKey, ConfigKind};
use crate::{ControlValue, Invention};

use super::config_diff::{
    configs_equal, control_updates_for, implicit_restores, stateless_keys, without,
};
use crate::invention::format::ModuleSpec;
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo, WrittenSince};

/// The runtime mutations that turn the current graph into the new document.
#[derive(Debug, Default)]
pub(crate) struct ReloadPlan {
    pub(crate) added: Vec<ModuleSpec>,
    pub(crate) removed: Vec<String>,
    pub(crate) swapped: Vec<ModuleSpec>,
    pub(crate) control_updates: Vec<(String, String, ControlValue)>,
    /// New configs for modules whose delta lands as control updates, so the
    /// runtime's stored config tracks the document and the next reload does
    /// not re-detect (and re-apply) the same delta.
    pub(crate) refreshed_configs: Vec<(String, serde_json::Value)>,
    pub(crate) removed_connections: Vec<RuntimeConnectionInfo>,
    pub(crate) added_connections: Vec<RuntimeConnectionInfo>,
    pub(crate) unchanged: Vec<String>,
}

/// A module type's config keys, and those of them only a reload reads
/// ([`crate::ModuleFactory::reload_keys`]).
pub(crate) type TypeKeys = (&'static [ConfigKey], &'static [&'static str]);

/// What a reload restores the controls a new config leaves implicit to.
pub(crate) struct Implicit<'a> {
    /// The authored writes the stored configs do not record.
    pub(crate) written: &'a WrittenSince,
    /// The value control `key` of module `id` has in a fresh build of the
    /// new document, when it has one.
    pub(crate) fresh: &'a dyn Fn(&str, &str) -> Option<ControlValue>,
}

/// Diffs the new document against the current runtime topology.
///
/// A module keeps its running instance (and therefore its phase and state)
/// when its id, type, and config are unchanged and its type's development
/// definition did not change. A config-only change becomes `set_control`
/// updates when every changed top-level key maps to a control the module
/// exposes; otherwise the module is swapped. A number compares by value
/// under the key type its module type declares (`config_keys`), so `440`
/// and `440.0` are no change; see [`ConfigKind::same_value`](crate::module_config::ConfigKind::same_value).
/// So does a key `control_kind` reports a running module declares as a
/// control. A key only a reload reads (the second slice `config_keys`
/// gives) never swaps the module: a change to it alone refreshes its
/// config. So does a key `is_event` reports names one of a running
/// module's event controls: an event holds no state for its config to
/// set, so a key naming one never fires on reload (a clock's reset on
/// reload is its own write, [`crate::ModuleFactory::writes_on_reload`]).
/// `has_control` answers whether a module exposes a control key at
/// runtime.
///
/// With `implicit`, a kept module's authored writes that its stored config
/// does not record (to a default, an indexed key, a key stored as a null or
/// array) and the new config does not state restore by control writes to
/// the control's value in a fresh build of that config (see
/// [`config_diff::implicit_restores`]), so the module sounds as the file
/// does without a rebuild. A write `fresh` gives no value for keeps playing
/// (a development instance's, one no write restores). A key the new config
/// removes returns the same way, to its fresh value, and rebuilds the
/// module only when `fresh` gives none. An edit batch passes none: its
/// document states every write it records, and removes no key from a
/// module it keeps.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_reload(
    current_modules: &IndexMap<String, RuntimeModuleInfo>,
    current_connections: &[RuntimeConnectionInfo],
    new: &Invention,
    changed_types: &HashSet<String>,
    config_keys: impl Fn(&str) -> TypeKeys,
    control_kind: impl Fn(&str, &str) -> Option<ConfigKind>,
    is_event: impl Fn(&str, &str) -> bool,
    mut has_control: impl FnMut(&str, &str) -> bool,
    implicit: Option<Implicit<'_>>,
) -> Result<ReloadPlan, String> {
    let mut plan = ReloadPlan::default();

    for spec in &new.modules {
        let (keys, reload_only) = config_keys(&spec.module_type);
        let kind = |key: &str| {
            let declared = keys.iter().find(|declared| declared.key == key);
            declared
                .map(|declared| declared.kind)
                .or_else(|| control_kind(&spec.id, key))
        };
        let fresh = |key: &str| {
            implicit
                .as_ref()
                .and_then(|implicit| (implicit.fresh)(&spec.id, key))
        };
        match current_modules.get(&spec.id) {
            None => plan.added.push(spec.clone()),
            Some(info)
                if info.module_type != spec.module_type
                    || changed_types.contains(&spec.module_type) =>
            {
                plan.swapped.push(spec.clone());
            }
            // Keys the running module holds no state for (those only a
            // reload reads, and its events) never rebuild or write it:
            // compared without them, and kept in its config when only
            // they changed.
            Some(info) => {
                let stateless = stateless_keys(reload_only, [&info.config, &spec.config], |key| {
                    is_event(&spec.id, key)
                });
                let previous = without(&info.config, &stateless);
                let config = without(&spec.config, &stateless);
                let wholes: Vec<&ConfigKey> = (keys.iter())
                    .filter(|declared| !declared.replaces.is_empty())
                    .filter(|declared| has_control(&spec.id, declared.key))
                    .collect();
                let restore = |key: &str| restore_write(&wholes, &config, key, &fresh);
                let mut updates = if configs_equal(&previous, &config, &kind) {
                    (!stateless.is_empty() && info.config != spec.config).then(Vec::new)
                } else {
                    let planned = control_updates_for(
                        &previous,
                        &config,
                        &kind,
                        |key| has_control(&spec.id, key),
                        &restore,
                    );
                    if planned.is_none() {
                        plan.swapped.push(spec.clone());
                        continue;
                    }
                    planned
                };
                if let Some(implicit) = &implicit {
                    let pending = updates.as_deref().unwrap_or_default();
                    let restores = implicit_restores(
                        implicit.written.unrecorded(&spec.id),
                        &config,
                        pending,
                        |key| is_event(&spec.id, key),
                        |key| has_control(&spec.id, key),
                        &restore,
                    );
                    if !restores.is_empty() {
                        updates.get_or_insert_with(Vec::new).extend(restores);
                    }
                }
                // As a cold build applies the config: a value replacing
                // others (a melody's whole scale) first, then every key it
                // replaces, changed or not (see `ConfigKey::replacing`).
                if let Some(updates) = &mut updates {
                    if !replay_replaced(keys, &config, updates, |key| has_control(&spec.id, key)) {
                        plan.swapped.push(spec.clone());
                        continue;
                    }
                    updates.sort_by_key(|(key, _)| replaced_by(keys, key).is_empty());
                }
                plan.unchanged.push(spec.id.clone());
                if let Some(updates) = updates {
                    plan.refreshed_configs
                        .push((spec.id.clone(), spec.config.clone()));
                    plan.control_updates.extend(
                        updates
                            .into_iter()
                            .map(|(key, value)| (spec.id.clone(), key, value)),
                    );
                }
            }
        }
    }

    let new_ids: HashSet<&str> = new.modules.iter().map(|spec| spec.id.as_str()).collect();
    plan.removed = current_modules
        .keys()
        .filter(|id| !new_ids.contains(id.as_str()))
        .cloned()
        .collect();

    let desired: Vec<RuntimeConnectionInfo> =
        new.connections
            .iter()
            .map(|conn| {
                Ok(RuntimeConnectionInfo {
                    from: conn.from.clone(),
                    from_port: conn.from_port.clone().ok_or_else(|| {
                        format!("Missing from_port in connection from {}", conn.from)
                    })?,
                    to: conn.to.clone(),
                    to_port: conn
                        .to_port
                        .clone()
                        .ok_or_else(|| format!("Missing to_port in connection to {}", conn.to))?,
                })
            })
            .collect::<Result<_, String>>()?;

    // Connections touching a removed module are cleaned up by remove_module,
    // so only surviving endpoints need explicit disconnects.
    let removed_ids: HashSet<&str> = plan.removed.iter().map(String::as_str).collect();
    plan.removed_connections = current_connections
        .iter()
        .filter(|conn| {
            !desired.contains(conn)
                && !removed_ids.contains(conn.from.as_str())
                && !removed_ids.contains(conn.to.as_str())
        })
        .cloned()
        .collect();
    plan.added_connections = desired
        .into_iter()
        .filter(|conn| !current_connections.contains(conn))
        .collect();

    Ok(plan)
}

/// The write that returns control `key` of a kept module to what a fresh
/// build of `config` plays. A key one of the module's `wholes` (its
/// replacing keys that are controls) replaces, a melody's `note_weight.3`
/// under `note_weights`, returns by that whole key, as `config` states it
/// or as built: the positions `config` states are written again after it
/// ([`replay_replaced`]), and nothing the module keeps of a write to the
/// position (a weight a whole scale keeps) outlives the file. Any other
/// key returns as built.
fn restore_write(
    wholes: &[&ConfigKey],
    config: &serde_json::Value,
    key: &str,
    fresh: &dyn Fn(&str) -> Option<ControlValue>,
) -> Option<(String, ControlValue)> {
    let replacing = |declared: &&&ConfigKey| declared.replaces.iter().any(|gone| names(gone, key));
    let Some(whole) = wholes.iter().find(replacing) else {
        return Some((key.to_string(), fresh(key)?));
    };
    // A structured key stated as structure is written as its JSON text; a
    // null reads as absent, as a build reads it.
    let value = match config.get(whole.key).filter(|value| !value.is_null()) {
        Some(value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)))
            if whole.kind == ConfigKind::Json =>
        {
            ControlValue::String(value.to_string())
        }
        Some(value) => scalar_control_value(value)?,
        None => fresh(whole.key)?,
    };
    Some((whole.key.to_string(), value))
}

/// Adds to `updates` the keys of `config` an update replaces among the
/// type's `keys`, unchanged as they may be (a melody's count and degrees
/// written within its scale), so they are written again after it, as a
/// cold build of `config` holds them. `false` when one is not a value a
/// control takes, or names no control: only a rebuild plays it as built.
fn replay_replaced(
    keys: &[ConfigKey],
    config: &serde_json::Value,
    updates: &mut Vec<(String, ControlValue)>,
    mut has_control: impl FnMut(&str) -> bool,
) -> bool {
    let mut replayed = Vec::new();
    for (key, _) in updates.iter() {
        for (name, value) in
            crate::invention::authored_document::replaced_in(config, replaced_by(keys, key))
        {
            let pending = |(written, _): &(String, ControlValue)| written == name;
            if updates.iter().any(pending) || replayed.iter().any(pending) {
                continue;
            }
            match scalar_control_value(value) {
                Some(value) if has_control(name) => replayed.push((name.clone(), value)),
                _ => return false,
            }
        }
    }
    updates.extend(replayed);
    true
}

pub(crate) fn scalar_control_value(value: &serde_json::Value) -> Option<ControlValue> {
    match value {
        serde_json::Value::Number(number) => Some(ControlValue::Number(number.as_f64()? as f32)),
        serde_json::Value::Bool(flag) => Some(ControlValue::Bool(*flag)),
        serde_json::Value::String(text) => Some(ControlValue::String(text.clone())),
        _ => None,
    }
}
