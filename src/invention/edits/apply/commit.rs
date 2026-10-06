//! Preparing a planned batch off the audio thread, then committing it as one
//! publication with its control writes and events.

use std::collections::{HashMap, HashSet};

use super::{check_writes, refused_write, Batch, Refused};
use crate::invention::edits::EditedCandidate;
use crate::invention::format::ModuleSpec;
use crate::invention::publish::{BuiltModule, GraphChange, PreparedChange};
use crate::invention::reload::ReloadPlan;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::rpc::{
    truncate_on_char_boundary, ApplyEditsReport, ControlWriteFailure, RpcError, WrittenControl,
    MODULE_ERROR_BYTES,
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
    /// leave. Changes nothing in the running invention; the modules are built
    /// for real, so a recorder the batch adds opens its file here.
    ///
    /// `base_generation` is read before planning; a change published since
    /// is [`Refused::Moved`].
    pub(super) fn prepare_edits(
        &self,
        base_generation: u64,
        batch: &Batch,
    ) -> Result<PreparedEdits, Refused> {
        let plan = super::plan::plan_edits(&self.state.lock().unwrap(), &batch.candidate)?;
        let build = |spec: &ModuleSpec| -> Result<BuiltModule, Refused> {
            let config = batch.resolved.get(&spec.id).unwrap_or(&spec.config);
            let module = GraphChange::build(
                &self.registry,
                self.sample_rate,
                &spec.id,
                &spec.module_type,
                config,
            )?;
            write_built(&module, &spec.id, &batch.candidate)?;
            Ok(module)
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
        let change = self.stage_plan(base_generation, &plan, swapped, added)?;
        // Survivors are written only at the commit; their throwaway copies
        // already hold the batch's writes.
        let mut directory = change.surfaces.clone();
        for (id, surface) in &batch.provisional {
            if directory.contains_key(id) && !plan_builds(&plan, id) {
                directory.insert(id.clone(), surface.clone());
            }
        }
        check_writes(&directory, &batch.candidate)?;
        Ok(PreparedEdits { change, plan })
    }

    /// Publishes a prepared batch with the candidate as the retained
    /// document, writes the batch's control values, and announces each
    /// control the batch wrote.
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
        let mut previous = None;
        let committed = self.live.commit_with(change, |state| {
            previous = state.document.replace(document.clone());
        })?;
        // The old document drops here, off the publisher's lock.
        drop(previous);

        // Added and rebuilt modules took the batch's writes when they were
        // built (see `write_built`); only survivors are written, right after
        // the publication is queued.
        let built: HashSet<&str> = plan
            .added
            .iter()
            .chain(&plan.swapped)
            .map(|spec| spec.id.as_str())
            .collect();
        let snapshot = self.snapshot();
        let mut report = report_for(&plan, batch.edit_count);
        // Every write, in batch order, so each lands on the module as the
        // writes before it left it, as they were checked. Each control is
        // then settled by its last write: an earlier failure a later write
        // made good is not a failure.
        let mut last: HashMap<(&str, &str), Option<String>> = HashMap::new();
        for candidate in &batch.candidate.control_writes {
            let write = &candidate.write;
            if built.contains(write.module_id.as_str()) {
                continue;
            }
            // Unrecorded: the retained document already holds the value.
            let written =
                snapshot.set_control_transient(&write.module_id, &write.key, write.value.clone());
            let outcome = written.err().map(|error| match error {
                GraphCommandError::ControlError(message) => message,
                other => other.to_string(),
            });
            last.insert((write.module_id.as_str(), write.key.as_str()), outcome);
        }
        // One report entry per control, and one event per control written,
        // with its final value.
        let mut actual = Vec::new();
        let mut announced = Vec::new();
        for candidate in batch.candidate.final_writes() {
            let write = &candidate.write;
            let failure = last
                .get(&(write.module_id.as_str(), write.key.as_str()))
                .cloned()
                .flatten();
            let Some(mut error) = failure else {
                report
                    .controls_written
                    .push(WrittenControl::new(&write.module_id, &write.key));
                // Recorded and announced as written, not read back: a read
                // can return a command's idle state (a trigger reads empty)
                // or a value another writer performed since.
                announced.push((write, write.value.clone()));
                continue;
            };
            if let Ok(value) = self.get_control(&write.module_id, &write.key) {
                actual.push((write, value));
            }
            truncate_on_char_boundary(&mut error, MODULE_ERROR_BYTES);
            report.controls_failed.push(ControlWriteFailure {
                edit_index: candidate.edit_index,
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                error,
            });
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
        for (write, value) in announced {
            snapshot.emit_control_changed(&write.module_id, &write.key, value);
        }
        Ok(report)
    }
}

/// Makes the batch's final writes to a module it adds or rebuilds on the
/// instance just built, before it is attached and prepared for publication,
/// so the prepared module already holds every value and the audio thread
/// adopts nothing new after the swap. A control's key need not be the config
/// key its module is built from (an oscillator's `type`, say), so the config
/// alone may not carry the value. A write the module refuses refuses the
/// batch at its edit, with nothing published.
fn write_built(
    module: &BuiltModule,
    id: &str,
    candidate: &EditedCandidate,
) -> Result<(), RpcError> {
    // Every write, in batch order, as they were checked. Each control is
    // settled by its last write: a failure a later write made good (a sample
    // replaced by one that loads) does not refuse the batch.
    let mut last: HashMap<&str, Option<String>> = HashMap::new();
    for candidate in &candidate.control_writes {
        let write = &candidate.write;
        if write.module_id != id {
            continue;
        }
        let surface = module
            .surface
            .as_ref()
            .ok_or_else(|| refused_write(candidate, "the module has no controls".to_string()))?;
        let outcome = surface.set_control(&write.key, write.value.clone()).err();
        last.insert(write.key.as_str(), outcome);
    }
    for candidate in candidate.final_writes() {
        if candidate.write.module_id != id {
            continue;
        }
        if let Some(Some(reason)) = last.get(candidate.write.key.as_str()) {
            return Err(refused_write(candidate, reason.clone()));
        }
    }
    Ok(())
}

/// Whether `plan` builds `id` afresh (added or rebuilt).
fn plan_builds(plan: &ReloadPlan, id: &str) -> bool {
    plan.added
        .iter()
        .chain(&plan.swapped)
        .any(|spec| spec.id == id)
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
