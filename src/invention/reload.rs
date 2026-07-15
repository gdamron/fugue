//! Glitch-free reload: diff a new invention document against the running
//! graph and apply the difference as runtime mutations.
//!
//! The entry point is [`RunningInvention::reload`]. It validates the whole
//! new document first (a throwaway build against a pristine registry), so an
//! invalid document leaves the running invention untouched and playback
//! continues on the last good version. Only after validation does it apply
//! the difference as add/remove/swap/connect/disconnect/set_control
//! mutations: modules unchanged by the diff keep their phase and state, and
//! the audio stream stays alive throughout.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

use crate::{ControlValue, Invention, ModuleRegistry};

use super::builder::{load_development_definition, resolve_invention_assets, InventionBuilder};
use super::format::ModuleSpec;
use super::runtime::{GraphCommandError, RunningInvention};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo};

/// Development definitions as loaded for a document, keyed by registered
/// type name and flattened across nesting (first registration wins, matching
/// builder semantics). Path-based definitions are captured at load time so a
/// later reload can detect that the file changed on disk.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentDefinitions {
    definitions: BTreeMap<String, Invention>,
}

impl DevelopmentDefinitions {
    /// Recursively loads every development definition reachable from a
    /// document, including path-based definitions nested inside other
    /// definitions. A name already collected is skipped, which both matches
    /// the builder's first-registration-wins semantics and guards against
    /// definition cycles.
    pub fn resolve(document: &Invention) -> Result<Self, Box<dyn std::error::Error>> {
        let mut definitions = BTreeMap::new();
        collect_definitions(document, &mut definitions)?;
        Ok(Self { definitions })
    }
}

fn collect_definitions(
    document: &Invention,
    definitions: &mut BTreeMap<String, Invention>,
) -> Result<(), Box<dyn std::error::Error>> {
    for spec in &document.developments {
        if definitions.contains_key(&spec.name) {
            continue;
        }
        let definition = load_development_definition(document, spec)?;
        definitions.insert(spec.name.clone(), definition.clone());
        collect_definitions(&definition, definitions)?;
    }
    Ok(())
}

/// Returns the development type names whose definitions changed between the
/// previously loaded document and the new one, transitively: a development
/// whose definition instantiates a changed type is itself changed. A name
/// with no known previous definition counts as changed, so an unknown
/// history degrades to conservatively rebuilding every development instance.
pub(crate) fn changed_development_types(
    previous: &DevelopmentDefinitions,
    new: &DevelopmentDefinitions,
) -> HashSet<String> {
    let mut changed: HashSet<String> = new
        .definitions
        .iter()
        .filter(|(name, definition)| previous.definitions.get(*name) != Some(definition))
        .map(|(name, _)| name.clone())
        .collect();

    loop {
        let mut grew = false;
        for (name, definition) in &new.definitions {
            if changed.contains(name) {
                continue;
            }
            if definition
                .modules
                .iter()
                .any(|module| changed.contains(&module.module_type))
            {
                changed.insert(name.clone());
                grew = true;
            }
        }
        if !grew {
            return changed;
        }
    }
}

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

/// Diffs the new document against the current runtime topology.
///
/// A module keeps its running instance (and therefore its phase and state)
/// when its id, type, and config are unchanged and its type's development
/// definition did not change. A config-only change becomes `set_control`
/// updates when every changed top-level key maps to a control the module
/// exposes; otherwise the module is swapped. `has_control` answers whether a
/// module exposes a control key at runtime.
pub(crate) fn plan_reload(
    current_modules: &IndexMap<String, RuntimeModuleInfo>,
    current_connections: &[RuntimeConnectionInfo],
    new: &Invention,
    changed_types: &HashSet<String>,
    mut has_control: impl FnMut(&str, &str) -> bool,
) -> Result<ReloadPlan, String> {
    let mut plan = ReloadPlan::default();

    for spec in &new.modules {
        match current_modules.get(&spec.id) {
            None => plan.added.push(spec.clone()),
            Some(info)
                if info.module_type != spec.module_type
                    || changed_types.contains(&spec.module_type) =>
            {
                plan.swapped.push(spec.clone());
            }
            Some(info) if !configs_equal(&info.config, &spec.config) => {
                match control_updates_for(&info.config, &spec.config, |key| {
                    has_control(&spec.id, key)
                }) {
                    Some(updates) => {
                        plan.unchanged.push(spec.id.clone());
                        plan.refreshed_configs
                            .push((spec.id.clone(), spec.config.clone()));
                        plan.control_updates.extend(
                            updates
                                .into_iter()
                                .map(|(key, value)| (spec.id.clone(), key, value)),
                        );
                    }
                    None => plan.swapped.push(spec.clone()),
                }
            }
            Some(_) => plan.unchanged.push(spec.id.clone()),
        }
    }

    let new_ids: HashSet<&str> = new.modules.iter().map(|spec| spec.id.as_str()).collect();
    plan.removed = current_modules
        .keys()
        .filter(|id| !new_ids.contains(id.as_str()))
        .cloned()
        .collect();

    let desired: Vec<RuntimeConnectionInfo> = new
        .connections
        .iter()
        .map(|conn| {
            Ok(RuntimeConnectionInfo {
                from: conn.from.clone(),
                from_port: conn
                    .from_port
                    .clone()
                    .ok_or_else(|| format!("Missing from_port in connection from {}", conn.from))?,
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

/// Treats a null config and an empty object as equivalent: omitting `config`
/// parses as `null`, while an explicit `{}` is an empty object.
fn configs_equal(previous: &serde_json::Value, new: &serde_json::Value) -> bool {
    previous == new || (is_empty_config(previous) && is_empty_config(new))
}

fn is_empty_config(value: &serde_json::Value) -> bool {
    value.is_null() || value.as_object().is_some_and(|map| map.is_empty())
}

/// Maps a config delta to control updates, or `None` when the delta cannot be
/// expressed as controls (a removed key, a non-scalar value, or a key the
/// module does not expose as a control) and the module must be swapped.
fn control_updates_for(
    previous: &serde_json::Value,
    new: &serde_json::Value,
    mut has_control: impl FnMut(&str) -> bool,
) -> Option<Vec<(String, ControlValue)>> {
    static EMPTY: std::sync::LazyLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::LazyLock::new(serde_json::Map::new);
    let previous = match previous {
        serde_json::Value::Null => &*EMPTY,
        other => other.as_object()?,
    };
    let new = match new {
        serde_json::Value::Null => &*EMPTY,
        other => other.as_object()?,
    };

    // A key that disappeared means "revert to the built-in default", which
    // only a rebuild of the module can express.
    if previous.keys().any(|key| !new.contains_key(key)) {
        return None;
    }

    let mut updates = Vec::new();
    for (key, value) in new {
        if previous.get(key) == Some(value) {
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

pub(crate) fn scalar_control_value(value: &serde_json::Value) -> Option<ControlValue> {
    match value {
        serde_json::Value::Number(number) => Some(ControlValue::Number(number.as_f64()? as f32)),
        serde_json::Value::Bool(flag) => Some(ControlValue::Bool(*flag)),
        serde_json::Value::String(text) => Some(ControlValue::String(text.clone())),
        _ => None,
    }
}

/// What a successful diff-applied reload did to the running graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ReloadReport {
    /// Module ids added to the graph.
    pub added: Vec<String>,
    /// Module ids removed from the graph.
    pub removed: Vec<String>,
    /// Module ids rebuilt in place (type, config, or development definition
    /// changed); their internal state restarts, everything else keeps playing.
    pub swapped: Vec<String>,
    /// Config deltas applied live as `module.key` control updates.
    pub controls_updated: Vec<String>,
    pub connections_added: usize,
    pub connections_removed: usize,
    /// Modules untouched by the diff; they keep their phase and state.
    pub unchanged: usize,
}

/// Why a reload did not apply.
#[derive(Debug)]
pub enum ReloadError {
    /// The new document failed validation or could not be built. The running
    /// invention was not touched; playback continues on the last good
    /// version.
    Invalid(String),
    /// The diff failed while being applied and the graph may be partially
    /// updated; the caller should fall back to a clean rebuild of the new
    /// document.
    Apply(GraphCommandError),
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "invalid invention: {reason}"),
            Self::Apply(error) => write!(formatter, "reload diff failed to apply: {error}"),
        }
    }
}

impl std::error::Error for ReloadError {}

impl RunningInvention {
    /// Reloads a new invention document into the running graph without
    /// restarting the audio stream.
    ///
    /// The document is validated with a full throwaway build first, so any
    /// parse, port, config, or development error returns
    /// [`ReloadError::Invalid`] with the running invention untouched. The
    /// validated document is then diffed against the current topology and
    /// applied as runtime mutations. Modules whose development definition
    /// changed (directly or through a nested development) are rebuilt;
    /// everything else keeps its state.
    ///
    /// On [`ReloadError::Apply`] the graph may be partially updated and the
    /// caller should fall back to a clean rebuild of the same document.
    pub fn reload(&mut self, invention: Invention) -> Result<ReloadReport, ReloadError> {
        // Kept as authored (assets unresolved) to become the retained
        // document once the diff applies.
        let document = invention.clone();
        let resolved = resolve_invention_assets(invention)
            .map_err(|error| ReloadError::Invalid(error.to_string()))?;
        let new_definitions = DevelopmentDefinitions::resolve(&resolved)
            .map_err(|error| ReloadError::Invalid(error.to_string()))?;

        // Validate the whole document before touching the running graph. The
        // build starts from the pristine base registry so a development
        // removed from the document is an error, exactly as on a cold load.
        // The built runtime is discarded; its registry carries the freshly
        // registered development factories the diff needs.
        let builder =
            InventionBuilder::with_registry(self.sample_rate, self.base_registry.clone());
        let (validated, _) = builder
            .build(resolved.clone())
            .map_err(|error| ReloadError::Invalid(error.to_string()))?;
        let new_registry: ModuleRegistry = validated.registry.clone();
        drop(validated);

        let changed_types =
            changed_development_types(&self.development_definitions, &new_definitions);

        let (current_modules, current_connections) = {
            let state = self.state.lock().unwrap();
            (state.modules.clone(), state.connections.clone())
        };
        let plan = plan_reload(
            &current_modules,
            &current_connections,
            &resolved,
            &changed_types,
            |module_id, key| self.get_control(module_id, key).is_ok(),
        )
        .map_err(ReloadError::Invalid)?;

        // Adopt the new registry and definitions so swapped and added
        // modules build against the new development factories.
        self.adopt_definitions(new_registry, new_definitions);

        let report = self.apply_reload_plan(plan)?;
        // The new document is now what the graph was built from. Applied
        // after the plan so plan mutations cannot leave stale specs in it;
        // on an apply error the caller rebuilds cleanly, which retains the
        // document through the normal build path.
        self.state.lock().unwrap().document = Some(document);
        Ok(report)
    }

    fn apply_reload_plan(&self, plan: ReloadPlan) -> Result<ReloadReport, ReloadError> {
        for conn in &plan.removed_connections {
            self.disconnect(&conn.from, &conn.from_port, &conn.to, &conn.to_port)
                .map_err(ReloadError::Apply)?;
        }
        for spec in &plan.swapped {
            self.swap_module(spec.id.clone(), &spec.module_type, &spec.config, true)
                .map_err(ReloadError::Apply)?;
        }
        for module_id in &plan.removed {
            self.remove_module(module_id.clone())
                .map_err(ReloadError::Apply)?;
        }
        for spec in &plan.added {
            self.add_module(spec.id.clone(), &spec.module_type, &spec.config)
                .map_err(ReloadError::Apply)?;
        }
        for conn in &plan.added_connections {
            self.connect(&conn.from, &conn.from_port, &conn.to, &conn.to_port)
                .map_err(ReloadError::Apply)?;
        }
        for (module_id, key, value) in &plan.control_updates {
            self.set_control(module_id, key, value.clone())
                .map_err(ReloadError::Apply)?;
        }
        if !plan.refreshed_configs.is_empty() {
            let mut state = self.state.lock().unwrap();
            for (module_id, config) in plan.refreshed_configs {
                if let Some(info) = state.modules.get_mut(&module_id) {
                    info.config = config;
                }
            }
        }

        Ok(ReloadReport {
            added: plan.added.iter().map(|spec| spec.id.clone()).collect(),
            removed: plan.removed,
            swapped: plan.swapped.iter().map(|spec| spec.id.clone()).collect(),
            controls_updated: plan
                .control_updates
                .iter()
                .map(|(module_id, key, _)| format!("{module_id}.{key}"))
                .collect(),
            connections_added: plan.added_connections.len(),
            connections_removed: plan.removed_connections.len(),
            unchanged: plan.unchanged.len(),
        })
    }
}

#[cfg(test)]
mod tests;
