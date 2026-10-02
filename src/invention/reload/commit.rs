//! Preparing and committing a reload plan as one atomic graph change.
//!
//! [`RunningInvention::prepare_plan`] does every fallible step off the audio
//! thread and changes nothing visible; [`RunningInvention::commit_prepared`]
//! publishes the result as one topology change and then updates the
//! runtime's mirrors, control values, registry, and document. Reload uses
//! both; other whole-graph edits can reuse them.

use super::{ControlFailure, DevelopmentDefinitions, ReloadPlan, ReloadReport};
use crate::invention::publish::{edge, GraphChange, PreparedChange};
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::{ControlValue, Invention, ModuleRegistry};

/// A plan prepared off the audio thread, ready to commit.
pub(crate) struct PreparedCommit {
    change: PreparedChange,
    /// Becomes the retained document on commit, when given.
    document: Option<Invention>,
    /// Registry and development definitions to adopt on commit, when the
    /// change was built against new ones.
    adopt: Option<(ModuleRegistry, DevelopmentDefinitions)>,
    /// Validated control writes on surviving modules, values coerced to
    /// their controls' kinds.
    control_updates: Vec<(String, String, ControlValue)>,
    refreshed_configs: Vec<(String, serde_json::Value)>,
    report: ReloadReport,
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
        adopt: Option<(ModuleRegistry, DevelopmentDefinitions)>,
    ) -> Result<PreparedCommit, GraphCommandError> {
        let registry = adopt
            .as_ref()
            .map_or(&self.registry, |(registry, _)| registry);
        let build = |spec: &crate::invention::ModuleSpec| {
            GraphChange::build(
                registry,
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
        let change = change.prepare()?;
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
            report,
        })
    }

    /// Publishes a prepared plan as one topology change, then commits the
    /// runtime's mirrors, adopts any new registry, writes the plan's control
    /// updates on surviving modules, retains the document, and starts or
    /// stops scripts and agents.
    ///
    /// Fails, with nothing changed, only when another change published since
    /// the plan was prepared ([`GraphCommandError::TopologyMoved`]) or the
    /// audio thread is gone. Control updates are written right after the
    /// publication is queued, so a value may be heard up to one block before
    /// the new topology. A validated update that still fails when written
    /// keeps the module's previous value, is moved from the report's
    /// `controls_updated` to its `controls_failed`, and that previous value
    /// is what the retained document records.
    pub(crate) fn commit_prepared(
        &mut self,
        prepared: PreparedCommit,
    ) -> Result<ReloadReport, GraphCommandError> {
        let committed = self.live.commit(prepared.change)?;
        if let Some((registry, definitions)) = prepared.adopt {
            self.adopt_definitions(registry, definitions);
        }

        let mut report = prepared.report;
        let snapshot = self.snapshot();
        let mut kept = Vec::new();
        for (module_id, key, value) in prepared.control_updates {
            // Quiet: carrying authored values into the rebuilt graph is
            // reconstruction, not a live agent-initiated change. The reload's
            // snapshot already conveys the new state, so emitting a
            // `ControlChanged` per carried value would be redundant and would
            // misattribute the reload as a conducting gesture.
            if let Err(error) = snapshot.set_control_recorded(&module_id, &key, value) {
                // The topology is already published; the module keeps its
                // previous value rather than failing a change that landed.
                let label = format!("{module_id}.{key}");
                report.controls_updated.retain(|updated| *updated != label);
                if let Ok(actual) = self.get_control(&module_id, &key) {
                    kept.push((module_id.clone(), key.clone(), actual));
                }
                report.controls_failed.push(ControlFailure {
                    module_id,
                    key,
                    error: match error {
                        GraphCommandError::ControlError(message) => message,
                        other => other.to_string(),
                    },
                });
            }
        }
        {
            let mut state = self.state.lock().unwrap();
            for (module_id, config) in prepared.refreshed_configs {
                if let Some(info) = state.modules.get_mut(&module_id) {
                    info.config = config;
                }
            }
            // Retained after the control writes so they cannot leave stale
            // values in the document the graph now reflects.
            if let Some(document) = prepared.document {
                state.document = Some(document);
            }
            for (module_id, key, actual) in &kept {
                state.document_write_control(module_id, key, actual);
            }
        }

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
