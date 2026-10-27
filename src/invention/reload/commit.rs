//! Preparing and committing a reload plan as one atomic graph change.
//!
//! [`RunningInvention::prepare_plan`] does every fallible step off the audio
//! thread and changes nothing visible; [`RunningInvention::commit_prepared`]
//! publishes the result as one topology change and then updates the
//! runtime's mirrors, control values, registry, and document. Reload uses
//! both; other whole-graph edits can reuse them.

use super::{ControlFailure, DevelopmentDefinitions, ReloadPlan, ReloadReport};
use crate::invention::publish::{edge, BuiltModule, GraphChange, PreparedChange};
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::rpc::{truncate_on_char_boundary, MODULE_ERROR_BYTES};
use crate::{ControlValue, Invention, ModuleRegistry};
use std::sync::Arc;

/// A plan prepared off the audio thread, ready to commit.
pub(crate) struct PreparedCommit {
    pub(super) change: PreparedChange,
    /// Becomes the retained document on commit, when given.
    pub(super) document: Option<Invention>,
    /// Registry and development definitions to adopt on commit, when the
    /// change was built against new ones.
    pub(super) adopt: Option<(Arc<ModuleRegistry>, DevelopmentDefinitions)>,
    /// Validated control writes on surviving modules, values coerced to
    /// their controls' kinds.
    pub(super) control_updates: Vec<(String, String, ControlValue)>,
    pub(super) refreshed_configs: Vec<(String, serde_json::Value)>,
    /// The lockfile that governed preparing it, which the graph is built
    /// under from the commit on.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) built_under: crate::pkg::lock::Governance,
    pub(super) report: ReloadReport,
}

impl PreparedCommit {
    /// The commit, planned from the retained document at `authored`, the
    /// epoch read before planning: authored writes recorded since are kept
    /// over its document (see `PreparedChange::authored_at`).
    pub(super) fn planned_at(mut self, authored: u64) -> Self {
        self.change.authored_at = Some(authored);
        self
    }
}

impl RunningInvention {
    /// Prepares `plan` without changing anything visible: builds added and
    /// swapped modules (against `adopt`'s registry when given, otherwise the
    /// current one), attaches schedulers against the directory as the plan
    /// will leave it, compiles the complete next topology, and validates
    /// every control update against that directory (see
    /// [`crate::ControlSurface::validate_control`]).
    ///
    /// `base_generation` is [`crate::invention::publish::LiveGraph::generation`]
    /// read before `plan` was made. When the graph has changed since, the
    /// plan is stale and preparation fails with
    /// [`GraphCommandError::TopologyMoved`]; a change after preparation is
    /// refused the same way on commit. Plan again from the same document.
    ///
    /// `document`, when given, becomes the retained document on commit.
    pub(crate) fn prepare_plan(
        &self,
        base_generation: u64,
        plan: ReloadPlan,
        document: Option<Invention>,
        adopt: Option<(Arc<ModuleRegistry>, DevelopmentDefinitions)>,
    ) -> Result<PreparedCommit, GraphCommandError> {
        let registry = match &adopt {
            Some((registry, _)) => registry.clone(),
            // Only a reload adopts a registry, and reloads are serialized
            // by `&mut self`, so this one stays current to the commit.
            None => self.registry(),
        };
        let build = |spec: &crate::invention::ModuleSpec| {
            GraphChange::build(
                &registry,
                self.sample_rate,
                &spec.id,
                &spec.module_type,
                &spec.config,
            )
        };
        let swapped = plan
            .swapped
            .iter()
            .map(build)
            .collect::<Result<Vec<_>, _>>()?;
        let added = plan
            .added
            .iter()
            .map(build)
            .collect::<Result<Vec<_>, _>>()?;

        let writes = plan
            .control_updates
            .iter()
            .map(|(id, key, value)| (id.as_str(), key.as_str(), value));
        let change = self.stage_plan(base_generation, &plan, swapped, added, writes)?;
        let control_updates = validate_control_updates(&change, plan.control_updates)?;

        let report = ReloadReport {
            added: plan.added.iter().map(|spec| spec.id.clone()).collect(),
            removed: plan.removed,
            swapped: plan.swapped.iter().map(|spec| spec.id.clone()).collect(),
            controls_updated: control_updates
                .iter()
                .map(|(module_id, key, _)| format!("{module_id}.{key}"))
                .collect(),
            controls_failed: Vec::new(),
            connections_added: plan.added_connections.len(),
            connections_removed: plan.removed_connections.len(),
            unchanged: plan.unchanged.len(),
        };
        Ok(PreparedCommit {
            change,
            document,
            adopt,
            control_updates,
            refreshed_configs: plan.refreshed_configs,
            #[cfg(not(target_arch = "wasm32"))]
            built_under: crate::pkg::lock::governing(),
            report,
        })
    }

    /// Applies `plan`'s structural changes to a change begun against the
    /// current topology and prepares it (see [`GraphChange::prepare`]).
    /// `swapped` and `added` are the modules built for `plan.swapped` and
    /// `plan.added`, in the same order. Control updates are the caller's to
    /// make, but `writes` names them, so a surviving scheduler whose target
    /// the plan rebuilds or removes re-resolves the schedule written to it
    /// (see [`GraphChange::reschedule`]).
    ///
    /// Fails with [`GraphCommandError::TopologyMoved`] when the graph has
    /// changed since `base_generation`, and with the preparation's error when
    /// a connection or schedule does not resolve; either way nothing visible
    /// changed.
    pub(crate) fn stage_plan<'a>(
        &self,
        base_generation: u64,
        plan: &ReloadPlan,
        swapped: Vec<BuiltModule>,
        added: Vec<BuiltModule>,
        writes: impl IntoIterator<Item = (&'a str, &'a str, &'a ControlValue)>,
    ) -> Result<PreparedChange, GraphCommandError> {
        let mut change = self.live.begin();
        if change.base_generation != base_generation {
            return Err(GraphCommandError::TopologyMoved);
        }
        for conn in &plan.removed_connections {
            change.disconnect(edge(&conn.from, &conn.from_port, &conn.to, &conn.to_port));
        }
        // A swap keeps the module's connections whose ports it still has.
        for (spec, module) in plan.swapped.iter().zip(swapped) {
            change.upsert(&spec.id, module);
        }
        for module_id in &plan.removed {
            change.remove(module_id);
        }
        for (spec, module) in plan.added.iter().zip(added) {
            change.upsert(&spec.id, module);
        }
        for conn in &plan.added_connections {
            change.connect(edge(&conn.from, &conn.from_port, &conn.to, &conn.to_port))?;
        }
        let mut written = Vec::new();
        for (id, key, value) in writes {
            if let ("schedule", Ok(json)) = (key, value.as_string()) {
                change.reschedule(id, json);
            }
            written.push((id.to_owned(), key.to_owned()));
        }
        let mut prepared = change.prepare()?;
        prepared.writes = written;
        Ok(prepared)
    }

    /// Publishes a prepared plan as one topology change, then commits the
    /// runtime's mirrors, adopts any new registry and definitions, writes
    /// the plan's control updates on surviving modules, retains the
    /// document, and starts or stops scripts and agents.
    ///
    /// Fails, with nothing changed, only when another change published since
    /// the plan was prepared ([`GraphCommandError::TopologyMoved`]), even one
    /// whose topology is unchanged, or the audio thread is gone. The
    /// document, refreshed configs, and new registry are adopted in the same
    /// step as the publication, so no other edit can land between them, and
    /// an edit that built against the previous registry is built again.
    /// Control updates are written right after the publication is queued. A
    /// declared control's update is a request behind it and never leads the
    /// new topology; an undeclared control's may lead by up to one block. A
    /// validated update that still fails when written keeps the module's
    /// previous value, is moved from the report's `controls_updated` to its
    /// `controls_failed`, and the retained document keeps what it authored
    /// for the control before the reload, so the next reload tries it again
    /// (see `RuntimeState::restore_authored_control`).
    pub(crate) fn commit_prepared(
        &mut self,
        prepared: PreparedCommit,
    ) -> Result<ReloadReport, GraphCommandError> {
        let PreparedCommit {
            change,
            document,
            adopt,
            control_updates,
            refreshed_configs,
            #[cfg(not(target_arch = "wasm32"))]
            built_under,
            mut report,
        } = prepared;
        let (registry, definitions) = adopt.unzip();
        let replacing = registry.as_deref().map(crate::registry::ReplacingKeys::of);
        // The document as it was before the commit, when the commit replaced
        // it, or the one it kept; read back only should a write fail when
        // made. `None` when there was none: a failing control is then left
        // unauthored, to its default.
        let (mut previous, replaces) = (None, document.is_some());
        // Authored writes wait until the carried values are written, as an
        // edit batch's do (see `commit_edits`).
        let live = self.live.clone();
        let authoring = live.authoring();
        let committed = self.live.commit_adopting(change, registry, |state| {
            if let Some(replacing) = replacing {
                state.replacing = replacing;
            }
            // Every module's config is the document's now, unchanged or
            // refreshed: none holds a write made since.
            if document.is_some() {
                state.written_since.clear();
            }
            for (module_id, config) in refreshed_configs {
                if let Some(info) = state.modules.get_mut(&module_id) {
                    info.config = config;
                }
            }
            if let Some(document) = document {
                previous = state.document.replace(document);
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                state.built_under = built_under;
            }
        })?;
        // Definitions are read only by the next reload's plan, and reloads
        // are serialized by `&mut self`, so they need not land under the
        // publisher with the registry.
        if let Some(definitions) = definitions {
            self.adopt_definitions(definitions);
        }

        let snapshot = self.snapshot();
        let mut kept = Vec::new();
        for (module_id, key, value) in control_updates {
            // Quiet: carrying authored values into the rebuilt graph is
            // reconstruction, not a live agent-initiated change. The reload's
            // snapshot already conveys the new state, so emitting a
            // `ControlChanged` per carried value would be redundant and would
            // misattribute the reload as a conducting gesture. Unrecorded:
            // the retained document already holds the authored value, and
            // the value is already coerced. Recorded as it applies, though,
            // if a timed write contests it.
            let contested = &committed.contested;
            let written = snapshot.set_control_committed((&module_id, &key), value, contested);
            if let Err(error) = written {
                // The topology is already published; the module keeps its
                // previous value rather than failing a change that landed.
                let label = format!("{module_id}.{key}");
                report.controls_updated.retain(|updated| *updated != label);
                // An event (a clock's reset on reload) fires; it holds no
                // value for the document to keep.
                let event = snapshot.is_event(&module_id, &key);
                if let (false, Ok(actual)) = (event, self.get_control(&module_id, &key)) {
                    kept.push((module_id.clone(), key.clone(), actual));
                }
                let mut error = match error {
                    GraphCommandError::ControlError(message) => message,
                    other => other.to_string(),
                };
                truncate_on_char_boundary(&mut error, MODULE_ERROR_BYTES);
                report.controls_failed.push(ControlFailure {
                    module_id,
                    key,
                    error,
                });
            }
        }
        drop(authoring);
        if !kept.is_empty() {
            // Per key, so an edit landing since the commit keeps its own
            // changes to the document.
            let mut state = self.state.lock().unwrap();
            if !replaces {
                previous = state.document.clone();
            }
            for (module_id, key, actual) in &kept {
                let control = (module_id.as_str(), key.as_str());
                state.restore_authored_control(previous.as_ref(), control, actual);
            }
        }
        // The old document drops here, off the publisher's lock.
        drop(previous);

        self.follow_up(committed);
        Ok(report)
    }
}

/// Coerces each control update to its control's kind and validates it
/// against the directory `change` leaves, failing on the first refusal.
fn validate_control_updates(
    change: &PreparedChange,
    updates: Vec<(String, String, ControlValue)>,
) -> Result<Vec<(String, String, ControlValue)>, GraphCommandError> {
    updates
        .into_iter()
        .map(|(module_id, key, value)| {
            let surface = change.surfaces.get(&module_id).ok_or_else(|| {
                GraphCommandError::ControlError(format!(
                    "{module_id}.{key}: module has no controls"
                ))
            })?;
            let value = surface.coerce_value(&key, value);
            surface
                .validate_control(&key, &value, &change.surfaces)
                .map_err(|error| {
                    GraphCommandError::ControlError(format!("{module_id}.{key}: {error}"))
                })?;
            Ok((module_id, key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests;
