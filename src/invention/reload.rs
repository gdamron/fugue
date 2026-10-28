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

use crate::{Invention, ModuleRegistry};

use super::builder::{resolve_invention_assets, InventionBuilder};
use super::runtime::{ControlSurfaceInstance, GraphCommandError, RunningInvention};

pub use developments::DevelopmentDefinitions;
pub(crate) use developments::{changed_development_types, LoadedDevelopments};
pub(crate) use plan::{plan_reload, scalar_control_value, Implicit, ReloadPlan};

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
    /// The control surfaces of the validation build, by module id: what a
    /// fresh build of the document starts each control at, for a reload to
    /// restore what the document leaves implicit (see [`Implicit`]).
    pub(crate) fresh: Arc<std::sync::Mutex<IndexMap<String, ControlSurfaceInstance>>>,
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
    /// module right after publication (an undeclared control's value may be
    /// heard up to one block before the new topology); one that still fails when written is listed
    /// in [`ReloadReport::controls_failed`].
    ///
    /// When another edit (a script's, say) changes the graph while the
    /// reload is planned or prepared, the reload is planned and prepared
    /// again from the same validated document, up to three times in all.
    /// Any error leaves the running invention untouched.
    pub fn reload(&mut self, invention: Invention) -> Result<ReloadReport, ReloadError> {
        self.reload_in(invention, &mut Ungoverned)
    }

    /// [`Self::reload`] for a session whose package resolution a lockfile
    /// may govern (see [`crate::pkg::lock`]): the document is validated and
    /// prepared under `lock` on this thread only, so the running session and
    /// its scripts keep the lock that governs them; `lock` becomes the
    /// session's as the reload commits, and the previous one is kept when it
    /// is refused.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn reload_governed(
        &mut self,
        invention: Invention,
        lock: crate::pkg::lock::Governance,
    ) -> Result<ReloadReport, ReloadError> {
        self.reload_in(invention, &mut governed::Candidate(lock))
    }

    fn reload_in(
        &mut self,
        invention: Invention,
        policy: &mut dyn ReloadPolicy,
    ) -> Result<ReloadReport, ReloadError> {
        let (mut invention, mut validated) = (Some(invention), None);
        policy.prepare(&mut || {
            validated = invention
                .take()
                .map(|invention| self.validate_document(invention))
        });
        let validated = validated.expect("prepare runs its body")?;
        let mut attempt = 1;
        loop {
            // Read before planning: the plan reads state that commits under
            // the publisher, so a change landing after this read is caught
            // when the prepared change publishes, at worst spuriously.
            let base = self.live.generation();
            let authored = self.live.authored_epoch();
            let mut prepared = None;
            policy.prepare(&mut || {
                prepared = Some(self.plan_document(&validated).map(|plan| {
                    let adopt = (validated.registry.clone(), validated.definitions.clone());
                    let document = Some(validated.document.clone());
                    let prepared = self.prepare_plan(base, plan, document, Some(adopt));
                    prepared.map(|prepared| prepared.planned_at(authored))
                }))
            });
            let prepared = prepared.expect("prepare runs its body")?;
            // The candidate's lock becomes the session's only once its graph
            // has: the graph's own modules and live edits load under the
            // lock it was built under meanwhile (see `BuildPolicy`).
            let result = prepared.and_then(|prepared| {
                let committed = self.commit_prepared(prepared);
                if committed.is_ok() {
                    policy.adopt();
                }
                committed
            });
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
        let invalid = |error: Box<dyn std::error::Error>| ReloadError::Invalid(error.to_string());
        let (resolved, loaded) = resolve_invention_assets(invention, None).map_err(invalid)?;
        let definitions = DevelopmentDefinitions::of(loaded.clone());

        // The built runtime is discarded but for its control surfaces.
        let (built, _) =
            InventionBuilder::with_registry(self.sample_rate, self.base_registry.for_validation())
                .with_loaded(loaded.clone())
                .build(resolved.clone())
                .map_err(invalid)?;
        let fresh = built.control_surfaces.clone();
        drop(built);
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
            fresh,
        })
    }

    /// Diffs a validated document against the current topology.
    pub(crate) fn plan_document(
        &self,
        validated: &ValidatedDocument,
    ) -> Result<ReloadPlan, ReloadError> {
        let changed_types =
            changed_development_types(&self.development_definitions, &validated.definitions);
        // The modules and the lock they were built under, read together.
        #[cfg(not(target_arch = "wasm32"))]
        let (current_modules, current_connections, written, built_under) = {
            let state = self.state.lock().unwrap();
            let (modules, connections) = (state.modules.clone(), state.connections.clone());
            let written = state.written_since.clone();
            (modules, connections, written, state.built_under.clone())
        };
        #[cfg(target_arch = "wasm32")]
        let (current_modules, current_connections, written) = {
            let state = self.state.lock().unwrap();
            let written = state.written_since.clone();
            (state.modules.clone(), state.connections.clone(), written)
        };
        // A development instance is not restored: its config is applied as
        // it binds, after a validation build reads it, and one key may set
        // several inner controls to different values.
        let developments: HashSet<&str> = (validated.resolved.modules.iter())
            .filter(|spec| {
                validated
                    .definitions
                    .loaded
                    .get(&spec.module_type)
                    .is_some()
            })
            .map(|spec| spec.id.as_str())
            .collect();
        let fresh = |module_id: &str, key: &str| {
            if developments.contains(module_id) {
                return None;
            }
            let surfaces = validated.fresh.lock().unwrap();
            config_diff::fresh_value(surfaces.get(module_id)?.as_ref(), key)
        };
        let mut plan = plan_reload(
            &current_modules,
            &current_connections,
            &validated.resolved,
            &changed_types,
            |module_type| {
                let registry = &validated.registry;
                let keys = registry.config_keys(module_type);
                (keys, registry.reload_keys(module_type))
            },
            |module_id, key| self.control_kind(module_id, key),
            |module_id, key| self.is_event_control(module_id, key),
            |module_id, key| self.reload_writes(module_id, key),
            Some(Implicit {
                written: &written,
                fresh: &fresh,
            }),
        )
        .map_err(ReloadError::Invalid)?;
        #[cfg(not(target_arch = "wasm32"))]
        governed::rebuild_under_another_policy(&mut plan, validated, &built_under);
        // A kept module's reload writes (a clock that resets on reload)
        // land after its config's, so they act on the new values. Only a
        // reload makes them: an edit batch keeps the clock's position, and
        // a module inside a development is not reached (its spec is the
        // development's).
        for spec in &validated.resolved.modules {
            if !plan.unchanged.contains(&spec.id) {
                continue;
            }
            let writes = validated
                .registry
                .writes_on_reload(&spec.module_type, &spec.config);
            plan.control_updates.extend(
                writes
                    .into_iter()
                    .map(|(key, value)| (spec.id.clone(), key.to_string(), value)),
            );
        }
        Ok(plan)
    }
}

pub(crate) mod commit;
mod config_diff;
mod developments;
mod plan;
mod timed;

#[cfg(test)]
mod tests;

/// How a reload governs package resolution: what its document is validated
/// and prepared under, and what the session adopts as it commits.
trait ReloadPolicy {
    fn prepare(&mut self, body: &mut dyn FnMut());
    fn adopt(&mut self) {}
}

/// Resolution as the session's governing lock, if any, has it.
struct Ungoverned;

impl ReloadPolicy for Ungoverned {
    fn prepare(&mut self, body: &mut dyn FnMut()) {
        body()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod governed;
