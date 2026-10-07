//! Glitch-free reload: diff a new invention document against the running
//! graph and apply the difference as one atomic graph change.
//!
//! The entry point is [`RunningInvention::reload`], in four steps:
//! validate (a throwaway build of the whole document against a pristine
//! registry), plan (diff it against the running topology), prepare (build
//! and compile the next topology off the audio thread), and commit (publish
//! it as one change, then update the runtime's mirrors). An invalid document
//! or a failed preparation leaves the running invention untouched and
//! playback continues on the last good version. Modules unchanged by the
//! diff keep their phase and state, and the audio stream stays alive
//! throughout.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

use crate::{ControlValue, Invention, ModuleRegistry};

use super::builder::{load_development_definition, resolve_invention_assets, InventionBuilder};
use super::format::ModuleSpec;
use super::runtime::{GraphCommandError, RunningInvention};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo};

/// Development definitions as loaded for a document, by declaration scope
/// (see [`LoadedDevelopments`]). Path-based definitions are captured at load
/// time so a later reload can detect that a file changed on disk, nested ones
/// included.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentDefinitions {
    loaded: Arc<LoadedDevelopments>,
}

impl DevelopmentDefinitions {
    /// Recursively loads every development definition reachable from a
    /// document, including path-based definitions nested inside other
    /// definitions, each within the scope that declares it.
    pub fn resolve(document: &Invention) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self::of(Arc::new(LoadedDevelopments::load(document)?)))
    }

    /// The definitions `loaded`, kept for a later reload to compare against.
    pub(crate) fn of(loaded: Arc<LoadedDevelopments>) -> Self {
        Self { loaded }
    }
}

/// The developments one document declares, each loaded with the ones its
/// own definition declares, in declaration order. A development built after
/// load (a nested one, or one an edit adds) builds from the declaration in
/// scope where it is used, as loaded, never from disk again. Two siblings
/// may each declare a different nested development under the same alias.
///
/// A declaration whose alias an enclosing scope already registered is not
/// loaded: the builder keeps the inherited factory and never reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LoadedDevelopments(IndexMap<String, LoadedDevelopment>);

/// One loaded development declaration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedDevelopment {
    pub(crate) definition: Invention,
    /// The developments `definition` declares.
    pub(crate) scope: Arc<LoadedDevelopments>,
}

impl LoadedDevelopments {
    /// Loads every development `document` declares, recursively.
    pub(crate) fn load(document: &Invention) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_within(document, &HashSet::new(), &mut Vec::new())
    }

    /// Loads `document`'s declarations, skipping those whose alias is in
    /// `inherited`: the registry a nested build starts from already has
    /// them, and the builder keeps those (see `register_developments`).
    fn load_within(
        document: &Invention,
        inherited: &HashSet<String>,
        ancestry: &mut Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut scope = IndexMap::new();
        for spec in &document.developments {
            if inherited.contains(&spec.name) || scope.contains_key(&spec.name) {
                continue;
            }
            let definition = load_development_definition(document, spec)?;
            // A development declared again inside its own definition is a
            // cycle: its scope is left empty there.
            let nested = if ancestry.contains(&spec.name) {
                Self::default()
            } else {
                // A development's factory starts from the registry as it was
                // when the development was registered: what this document
                // inherits, and the developments it declares before this one.
                let mut visible = inherited.clone();
                visible.extend(scope.keys().cloned());
                ancestry.push(spec.name.clone());
                let nested = Self::load_within(&definition, &visible, ancestry);
                ancestry.pop();
                nested?
            };
            let loaded = LoadedDevelopment {
                definition,
                scope: Arc::new(nested),
            };
            scope.insert(spec.name.clone(), loaded);
        }
        Ok(Self(scope))
    }

    /// The declaration of `name` in this scope.
    pub(crate) fn get(&self, name: &str) -> Option<&LoadedDevelopment> {
        self.0.get(name)
    }
}

/// Returns the top-level development type names whose definitions changed
/// between the previously loaded document and the new one: a development
/// changes when its definition or any development it declares (in its own
/// scope, recursively) changed, or, transitively, when it instantiates a
/// changed top-level development it inherits. A name with no known previous
/// definition counts as changed, so an unknown history degrades to
/// conservatively rebuilding every development instance.
pub(crate) fn changed_development_types(
    previous: &DevelopmentDefinitions,
    new: &DevelopmentDefinitions,
) -> HashSet<String> {
    let mut changed: HashSet<String> = new
        .loaded
        .0
        .iter()
        .filter(|(name, loaded)| previous.loaded.get(name) != Some(*loaded))
        .map(|(name, _)| name.clone())
        .collect();

    loop {
        let mut grew = false;
        for (name, loaded) in &new.loaded.0 {
            if !changed.contains(name) && uses_any(loaded, &changed) {
                changed.insert(name.clone());
                grew = true;
            }
        }
        if !grew {
            return changed;
        }
    }
}

/// Whether `loaded`, or any development it declares, instantiates one of
/// `names` as inherited from an enclosing scope rather than declared locally.
fn uses_any(loaded: &LoadedDevelopment, names: &HashSet<String>) -> bool {
    let visible: HashSet<String> = names
        .iter()
        .filter(|name| loaded.scope.get(name).is_none())
        .cloned()
        .collect();
    loaded
        .definition
        .modules
        .iter()
        .any(|module| visible.contains(&module.module_type))
        || loaded
            .scope
            .0
            .values()
            .any(|nested| uses_any(nested, &visible))
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
    /// Config-as-control updates that passed validation but failed when
    /// written (a sample that did not load, say). The module keeps its
    /// previous value, and the retained document records that value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls_failed: Vec<ControlFailure>,
    pub connections_added: usize,
    pub connections_removed: usize,
    /// Modules untouched by the diff; they keep their phase and state.
    pub unchanged: usize,
}

/// A control write a reload could not make.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlFailure {
    pub module_id: String,
    pub key: String,
    /// The module's reason, cut to at most 256 bytes on a UTF-8 character
    /// boundary.
    pub error: String,
}

/// Why a reload did not apply. Either way the running invention is
/// untouched and playback continues on the last good version.
#[derive(Debug)]
pub enum ReloadError {
    /// The new document failed validation or could not be built.
    Invalid(String),
    /// The validated document's difference could not be prepared (a module
    /// failed to build or a schedule failed to resolve against the new
    /// graph) or could not be delivered because the audio thread is gone.
    /// Preparation precedes any change, and delivery is all-or-nothing, so
    /// nothing was applied.
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

/// How many times [`RunningInvention::reload`] (and an `ApplyEdits` batch)
/// plans and prepares its document when other edits keep changing the graph
/// underneath it.
pub(crate) const RELOAD_ATTEMPTS: usize = 3;

/// A new document that passed a full validation build, with what the
/// running invention adopts if the reload commits.
pub(crate) struct ValidatedDocument {
    /// The document as authored (assets unresolved), to retain.
    pub(crate) document: Invention,
    /// The document with assets resolved, to diff.
    pub(crate) resolved: Invention,
    /// The registry with the document's development factories registered.
    pub(crate) registry: Arc<ModuleRegistry>,
    /// The document's development definitions as loaded.
    pub(crate) definitions: DevelopmentDefinitions,
}

impl RunningInvention {
    /// Reloads a new invention document into the running graph without
    /// restarting the audio stream.
    ///
    /// The document is validated with a full throwaway build first, so any
    /// parse, port, config, or development error returns
    /// [`ReloadError::Invalid`] with the running invention untouched. The
    /// validated document is then diffed against the current topology,
    /// prepared off the audio thread, and published as one change: no audio
    /// block ever plays part of it. Modules whose development definition
    /// changed (directly or through a nested development) are rebuilt;
    /// everything else keeps its state. A config change expressible as
    /// controls is validated with the rest and written to the surviving
    /// module right after publication, so it may be heard up to one block
    /// before the new topology; one that still fails when written is listed
    /// in [`ReloadReport::controls_failed`].
    ///
    /// When another edit (a script's, say) changes the graph while the
    /// reload is planned or prepared, the reload is planned and prepared
    /// again from the same validated document, up to three times in all.
    /// Any error leaves the running invention untouched.
    pub fn reload(&mut self, invention: Invention) -> Result<ReloadReport, ReloadError> {
        let validated = self.validate_document(invention)?;
        let mut attempt = 1;
        loop {
            // Read before planning: the plan reads state that commits under
            // the publisher, so a change landing after this read is caught
            // when the prepared change publishes, at worst spuriously.
            let base = self.live.generation();
            let plan = self.plan_document(&validated)?;
            let adopt = (validated.registry.clone(), validated.definitions.clone());
            let result = self
                .prepare_plan(base, plan, Some(validated.document.clone()), Some(adopt))
                .and_then(|prepared| self.commit_prepared(prepared));
            match result {
                Err(GraphCommandError::TopologyMoved) if attempt < RELOAD_ATTEMPTS => attempt += 1,
                result => return result.map_err(ReloadError::Apply),
            }
        }
    }

    /// Validates a new document with a full throwaway build against the
    /// pristine base registry, so a development removed from the document is
    /// an error exactly as on a cold load. The build is in validation mode,
    /// so nothing it builds activates an output: a recorder that is already
    /// running is not built again over the file it is writing. Changes
    /// nothing.
    pub(crate) fn validate_document(
        &self,
        invention: Invention,
    ) -> Result<ValidatedDocument, ReloadError> {
        let document = invention.clone();
        let resolved = resolve_invention_assets(invention)
            .map_err(|error| ReloadError::Invalid(error.to_string()))?;
        let invalid = |error: Box<dyn std::error::Error>| ReloadError::Invalid(error.to_string());
        let loaded = Arc::new(LoadedDevelopments::load(&resolved).map_err(invalid)?);
        let definitions = DevelopmentDefinitions::of(loaded.clone());

        // The built runtime is discarded.
        InventionBuilder::with_registry(self.sample_rate, self.base_registry.for_validation())
            .with_loaded(loaded.clone())
            .build(resolved.clone())
            .map_err(invalid)?;
        // What the reload adopts: the document's development factories,
        // registered live from the same loaded definitions.
        let registry: ModuleRegistry =
            InventionBuilder::with_registry(self.sample_rate, self.base_registry.clone())
                .with_loaded(loaded)
                .register_developments_only(&resolved)
                .map_err(invalid)?;

        Ok(ValidatedDocument {
            document,
            resolved,
            registry: Arc::new(registry),
            definitions,
        })
    }

    /// Diffs a validated document against the current topology.
    pub(crate) fn plan_document(
        &self,
        validated: &ValidatedDocument,
    ) -> Result<ReloadPlan, ReloadError> {
        let changed_types =
            changed_development_types(&self.development_definitions, &validated.definitions);
        let (current_modules, current_connections) = {
            let state = self.state.lock().unwrap();
            (state.modules.clone(), state.connections.clone())
        };
        plan_reload(
            &current_modules,
            &current_connections,
            &validated.resolved,
            &changed_types,
            |module_id, key| self.get_control(module_id, key).is_ok(),
        )
        .map_err(ReloadError::Invalid)
    }
}

pub(crate) mod commit;

#[cfg(test)]
mod tests;
