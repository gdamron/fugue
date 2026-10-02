//! Preparing a planned batch off the audio thread, then committing it as one
//! publication with its control writes and events.

use std::collections::HashSet;

use super::{Batch, Refused};
use crate::invention::edits::EditedCandidate;
use crate::invention::format::ModuleSpec;
use crate::invention::publish::{BuiltModule, GraphChange, PreparedChange};
use crate::invention::reload::ReloadPlan;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::rpc::{
    truncate_on_char_boundary, ApplyEditsReport, ControlWriteFailure, EditFailure,
    EditFailureReason, EditOp, RpcError, WrittenControl, MODULE_ERROR_BYTES,
};

/// A batch's change, prepared and checked, ready to publish.
pub(super) struct PreparedEdits {
    change: PreparedChange,
    plan: ReloadPlan,
}

impl RunningInvention {
    /// Plans the batch against the graph as it is now, builds its added and
    /// rebuilt modules from their final configs, prepares the next topology,
    /// and checks every control write against the directory the change will
    /// leave. Changes nothing visible.
    ///
    /// `base_generation` is read before planning; a change published since
    /// is [`Refused::Moved`].
    pub(super) fn prepare_edits(
        &self,
        base_generation: u64,
        batch: &mut Batch,
    ) -> Result<PreparedEdits, Refused> {
        let plan = super::plan::plan_edits(&self.state.lock().unwrap(), &batch.candidate)?;
        let mut build = |spec: &ModuleSpec| -> Result<BuiltModule, GraphCommandError> {
            let config = batch.resolved.get(&spec.id).unwrap_or(&spec.config);
            if let Some(module) = batch.kept.take(&spec.id, &spec.module_type, config) {
                return Ok(module);
            }
            GraphChange::build(
                &self.registry,
                self.sample_rate,
                &spec.id,
                &spec.module_type,
                config,
            )
        };
        let swapped = plan
            .swapped
            .iter()
            .map(&mut build)
            .collect::<Result<Vec<_>, _>>()?;
        let added = plan
            .added
            .iter()
            .map(&mut build)
            .collect::<Result<Vec<_>, _>>()?;
        let change = self.stage_plan(base_generation, &plan, swapped, added)?;
        check_writes(&change, &batch.candidate)?;
        Ok(PreparedEdits { change, plan })
    }

    /// Publishes a prepared batch with the candidate as the retained
    /// document, writes the batch's control values on surviving modules,
    /// and announces each control the batch wrote.
    ///
    /// Fails, with nothing changed, only when another change published
    /// since the batch was prepared, or the audio thread is gone. A write
    /// that still fails when made is reported, and the module's actual value
    /// is written back to the retained document.
    pub(super) fn commit_edits(
        &self,
        prepared: PreparedEdits,
        batch: &Batch,
    ) -> Result<ApplyEditsReport, Refused> {
        let PreparedEdits { change, plan } = prepared;
        let document = &batch.candidate.document;
        let committed = self
            .live
            .commit_with(change, |state| state.document = Some(document.clone()))?;

        // Added and rebuilt modules were built from configs that hold their
        // values; only survivors are written, right after the publication
        // is queued.
        let built: HashSet<&str> = plan
            .added
            .iter()
            .chain(&plan.swapped)
            .map(|spec| spec.id.as_str())
            .collect();
        let snapshot = self.snapshot();
        let mut report = report_for(&plan, batch.edit_count);
        let mut announced = Vec::new();
        let mut actual = Vec::new();
        for candidate in batch.candidate.final_writes() {
            let write = &candidate.write;
            if !built.contains(write.module_id.as_str()) {
                // Unrecorded: the retained document already holds the value.
                let written = snapshot.set_control_transient(
                    &write.module_id,
                    &write.key,
                    write.value.clone(),
                );
                if let Err(error) = written {
                    if let Ok(value) = self.get_control(&write.module_id, &write.key) {
                        actual.push((write, value));
                    }
                    let mut error = match error {
                        GraphCommandError::ControlError(message) => message,
                        other => other.to_string(),
                    };
                    truncate_on_char_boundary(&mut error, MODULE_ERROR_BYTES);
                    report.controls_failed.push(ControlWriteFailure {
                        edit_index: candidate.edit_index,
                        module_id: write.module_id.clone(),
                        key: write.key.clone(),
                        error,
                    });
                    continue;
                }
            }
            report
                .controls_written
                .push(WrittenControl::new(&write.module_id, &write.key));
            announced.push(write);
        }
        if !actual.is_empty() {
            // Per key, so an edit landing since the commit keeps its own
            // changes to the document.
            let mut state = self.state.lock().unwrap();
            for (write, value) in &actual {
                state.document_write_control(&write.module_id, &write.key, value);
            }
        }

        self.follow_up(committed);
        for write in announced {
            snapshot.emit_control_changed(&write.module_id, &write.key, write.value.clone());
        }
        Ok(report)
    }
}

/// Checks every write the batch makes against the directory `change` leaves,
/// survivors and new modules alike, refusing the first the module's setter
/// would refuse at its edit's index. Every write is checked, not only each
/// control's last one, so an edit the module refuses fails the batch even
/// when a later edit overwrites it.
fn check_writes(change: &PreparedChange, candidate: &EditedCandidate) -> Result<(), RpcError> {
    for candidate in &candidate.control_writes {
        let write = &candidate.write;
        let refuse = |mut reason: String| {
            truncate_on_char_boundary(&mut reason, MODULE_ERROR_BYTES);
            RpcError::invalid_edit(EditFailure::new(
                candidate.edit_index,
                EditOp::SetControl,
                EditFailureReason::InvalidControlValue,
                format!(
                    "module '{}' refused the value for control '{}': {reason}",
                    write.module_id, write.key
                ),
            ))
        };
        let surface = change
            .surfaces
            .get(&write.module_id)
            .ok_or_else(|| refuse("the module has no controls".to_string()))?;
        surface
            .validate_control(&write.key, &write.value, &change.surfaces)
            .map_err(refuse)?;
    }
    Ok(())
}

/// The report's structural part, from the plan.
fn report_for(plan: &ReloadPlan, edit_count: usize) -> ApplyEditsReport {
    let ids = |specs: &[ModuleSpec]| specs.iter().map(|spec| spec.id.clone()).collect();
    ApplyEditsReport {
        edit_count,
        added: ids(&plan.added),
        removed: plan.removed.clone(),
        rebuilt: ids(&plan.swapped),
        controls_written: Vec::new(),
        controls_failed: Vec::new(),
        connections_added: plan.added_connections.len(),
        connections_removed: plan.removed_connections.len(),
        untouched: plan.unchanged.len(),
    }
}
