//! Preparing and committing a reload plan as one atomic graph change.
//!
//! [`RunningInvention::prepare_plan`] does every fallible step off the audio
//! thread and changes nothing visible; [`RunningInvention::commit_prepared`]
//! publishes the result as one topology change and then updates the
//! runtime's mirrors, control values, registry, and document. Reload uses
//! both; other whole-graph edits can reuse them.

use super::{DevelopmentDefinitions, ReloadPlan, ReloadReport};
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
    control_updates: Vec<(String, String, ControlValue)>,
    refreshed_configs: Vec<(String, serde_json::Value)>,
    report: ReloadReport,
}

impl RunningInvention {
    /// Prepares `plan` without changing anything visible: builds added and
    /// swapped modules (against `adopt`'s registry when given, otherwise the
    /// current one), attaches schedulers against the directory as the plan
    /// will leave it, and compiles the complete next topology.
    ///
    /// `document`, when given, becomes the retained document on commit.
    pub(crate) fn prepare_plan(
        &self,
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

        let report = ReloadReport {
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
        };
        Ok(PreparedCommit {
            change,
            document,
            adopt,
            control_updates: plan.control_updates,
            refreshed_configs: plan.refreshed_configs,
            report,
        })
    }

    /// Publishes a prepared plan as one topology change, then commits the
    /// runtime's mirrors, adopts any new registry, writes the plan's control
    /// updates on surviving modules, retains the document, and starts or
    /// stops scripts and agents.
    ///
    /// Control updates are written right after the publication is queued, so
    /// a value may be heard up to one block before the new topology. Fails
    /// only when the audio thread is gone, with nothing changed.
    pub(crate) fn commit_prepared(
        &mut self,
        prepared: PreparedCommit,
    ) -> Result<ReloadReport, GraphCommandError> {
        let committed = self.live.commit(prepared.change)?;
        if let Some((registry, definitions)) = prepared.adopt {
            self.adopt_definitions(registry, definitions);
        }

        let snapshot = self.snapshot();
        for (module_id, key, value) in prepared.control_updates {
            // Quiet: carrying authored values into the rebuilt graph is
            // reconstruction, not a live agent-initiated change. The reload's
            // snapshot already conveys the new state, so emitting a
            // `ControlChanged` per carried value would be redundant and would
            // misattribute the reload as a conducting gesture.
            if let Err(error) = snapshot.set_control_recorded(&module_id, &key, value) {
                // The topology is already published; a value the module
                // rejects keeps its previous setting rather than failing a
                // change that has landed.
                eprintln!("Warning: reload could not set {module_id}.{key}: {error}");
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
        }

        self.follow_up(committed);
        Ok(prepared.report)
    }
}

#[cfg(test)]
mod tests;
