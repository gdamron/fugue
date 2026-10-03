//! `ApplyEdits` on a running invention: a batch of structural edits checked,
//! validated, planned, prepared and published as one change, or refused with
//! nothing changed.
//!
//! The steps, each on the calling (control) thread:
//!
//! 1. [`check_edit_batch`]: the batch's size and non-finite numbers.
//! 2. [`apply_to_candidate`]: the edits, in order, on a copy of the retained
//!    authored document, checked against the running modules' ports and
//!    controls and the current registry.
//! 3. A throwaway build of the whole candidate with the registry as loaded,
//!    so development definitions are never read from disk again, in
//!    validation mode, so it activates no output (a recording's file, say).
//! 4. A plan from the retained document to the candidate (see
//!    [`plan::plan_edits`]), refused if it reaches past the batch.
//! 5. Preparation: added and rebuilt modules built from their final
//!    configs and given the batch's writes, the next topology compiled,
//!    every control write checked.
//! 6. Commit: one publication, the candidate retained with it, then the
//!    control writes on survivors and one `ControlChanged` per control.
//!    Added and rebuilt modules took theirs in step 5, when built.
//!
//! When another change publishes during steps 4 to 6 (a script's edit, say),
//! the batch is planned and prepared again from the same candidate, as a
//! reload is.

mod commit;
mod facts;
mod plan;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use super::{apply_to_candidate, CandidateWrite, EditedCandidate};
use crate::invention::builder::InventionBuilder;
use crate::invention::format::Invention;
use crate::invention::reload::RELOAD_ATTEMPTS;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::rpc::{
    check_edit_batch, truncate_on_char_boundary, ApplyEditsReport, EditFailure, EditFailureReason,
    EditOp, RpcError, RpcErrorCode, StructuralEdit, MODULE_ERROR_BYTES,
};
use crate::traits::ControlSurfaceMap;
use facts::LiveFacts;

/// A batch that passed its checks and validation, ready to plan.
pub(super) struct Batch {
    edit_count: usize,
    candidate: EditedCandidate,
    /// Each candidate module's config with its assets resolved, as the
    /// validation build read it: what added and rebuilt modules are built
    /// from.
    resolved: HashMap<String, serde_json::Value>,
}

/// Why one attempt at a batch did not commit.
pub(super) enum Refused {
    /// Another change published while the attempt was planned or prepared.
    Moved,
    /// The batch is refused; nothing changed.
    Batch(RpcError),
}

impl From<RpcError> for Refused {
    fn from(error: RpcError) -> Self {
        Self::Batch(error)
    }
}

impl From<GraphCommandError> for Refused {
    fn from(error: GraphCommandError) -> Self {
        match error {
            GraphCommandError::TopologyMoved => Self::Moved,
            other => Self::Batch(graph_refusal(other)),
        }
    }
}

/// The refusal for a batch whose change could not be prepared or published.
///
/// - [`GraphCommandError::AudioThreadStopped`] is `audio_thread_stopped`.
/// - [`GraphCommandError::TopologyMoved`] (still moving after every attempt)
///   and [`GraphCommandError::QueueFull`] are `internal`.
/// - Anything else (a module that does not build, a connection or schedule
///   that does not resolve) is `module_build_failed`, with no edit detail:
///   every edit applied, so no single one is to blame.
pub(crate) fn graph_refusal(error: GraphCommandError) -> RpcError {
    match error {
        GraphCommandError::AudioThreadStopped
        | GraphCommandError::TopologyMoved
        | GraphCommandError::QueueFull => RpcError::from(error),
        other => build_failed("could not be prepared", other.to_string()),
    }
}

/// A `module_build_failed` refusal, with the module's reason cut to about
/// [`MODULE_ERROR_BYTES`].
fn build_failed(what: &str, mut reason: String) -> RpcError {
    truncate_on_char_boundary(&mut reason, MODULE_ERROR_BYTES);
    RpcError::new(
        RpcErrorCode::ModuleBuildFailed,
        format!("the edited invention {what}: {reason}; nothing was applied"),
    )
}

/// Checks every write the batch makes against `surfaces`, the directory as
/// the batch leaves it, survivors and new modules alike, refusing the first
/// the module's setter would refuse at its edit's index. Every write is
/// checked, not only each control's last one, so an edit the module refuses
/// fails the batch even when a later edit overwrites it.
fn check_writes(surfaces: &ControlSurfaceMap, candidate: &EditedCandidate) -> Result<(), RpcError> {
    for candidate in &candidate.control_writes {
        let write = &candidate.write;
        let refuse = |reason: String| refused_write(candidate, reason);
        let surface = surfaces
            .get(&write.module_id)
            .ok_or_else(|| refuse("the module has no controls".to_string()))?;
        surface
            .validate_control(&write.key, &write.value, surfaces)
            .map_err(refuse)?;
    }
    Ok(())
}

/// The `invalid_edit` refusal for a control write the module refused, at
/// its edit's index.
fn refused_write(candidate: &CandidateWrite, mut reason: String) -> RpcError {
    let write = &candidate.write;
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
}

impl RunningInvention {
    /// Applies a batch of structural edits as one change: every edit lands
    /// in one publication and one retained document, or none does.
    ///
    /// The batch is applied to a copy of the retained authored document and
    /// validated as a whole before anything is published. Modules the batch
    /// does not touch keep their instance and phase. Control values written
    /// to surviving modules are applied right after the publication is
    /// queued, so one may be heard up to one block before the new topology.
    /// Added and rebuilt modules are built from their final configs, and the
    /// batch's writes to them are made through their setters as soon as they
    /// are built, before they are prepared: a control's key need not be the
    /// config key its module is built from. After the commit,
    /// one `ControlChanged` per control the batch wrote is announced to the
    /// event sink. The caller announces the new topology after that.
    ///
    /// Refusals change nothing (no module, connection, control, document or
    /// event):
    ///
    /// - `invalid_request`: the batch is empty or too large.
    /// - `invalid_edit`, with the edit's index: an edit cannot apply, or a
    ///   control value is one the module refuses.
    /// - `module_build_failed`, with no edit detail: the edited invention
    ///   does not build, or its change cannot be prepared.
    /// - `unsupported`: the invention keeps no authored document to edit.
    /// - `audio_thread_stopped`: the audio thread is gone.
    /// - `internal`: the invention changed underneath the batch in a way the
    ///   batch would undo, or kept changing through every attempt.
    ///
    /// A control write that passed every check but still fails when made
    /// does not refuse the batch: it is listed in
    /// [`ApplyEditsReport::controls_failed`] and the module's actual value is
    /// written back to the retained document.
    ///
    /// The batch is planned from the retained authored document, not from
    /// the configs the runtime built its modules from (see
    /// `docs/apply-edits.md`). When another edit changes the graph while the
    /// batch is planned or prepared, it is planned and prepared again from
    /// the same candidate, up to three times in all.
    pub fn apply_edits(&mut self, edits: &[StructuralEdit]) -> Result<ApplyEditsReport, RpcError> {
        check_edit_batch(edits)?;
        let document = self.state.lock().unwrap().document().ok_or_else(|| {
            RpcError::new(
                RpcErrorCode::Unsupported,
                "the running invention keeps no authored document to edit; nothing was applied",
            )
        })?;
        let mut facts = LiveFacts::new(self, &document);
        let candidate =
            apply_to_candidate(&document, edits, &mut facts).map_err(RpcError::invalid_edit)?;
        // Checked first against the directory as the batch will leave it, so
        // a value the module refuses is refused at its edit even where
        // building the edited invention would fail on it (an option written
        // into a config the module type parses). Checked again against the
        // prepared directory before publishing.
        check_writes(&facts.directory_after(&candidate.document), &candidate)?;
        let resolved = self.validate_candidate(&candidate.document)?;
        let batch = Batch {
            edit_count: edits.len(),
            candidate,
            resolved,
        };

        let mut attempt = 1;
        loop {
            // Read before planning, as reload does: a change landing after
            // this read is caught when the prepared change publishes.
            let base = self.live.generation();
            let result = self
                .prepare_edits(base, &batch)
                .and_then(|prepared| self.commit_edits(prepared, &batch));
            match result {
                Ok(report) => return Ok(report),
                Err(Refused::Moved) if attempt < RELOAD_ATTEMPTS => attempt += 1,
                Err(Refused::Moved) => return Err(graph_refusal(GraphCommandError::TopologyMoved)),
                Err(Refused::Batch(error)) => return Err(error),
            }
        }
    }

    /// Builds the whole candidate and throws the build away, returning each
    /// module's config with its assets resolved. Built with the registry as
    /// loaded, which already carries every development's factory, so the
    /// candidate's developments are not registered (or read from disk)
    /// again, nested ones included. Built in validation mode, so no module
    /// activates an output: a sink recording to a file is not built again
    /// over the file it is writing. Changes nothing.
    fn validate_candidate(
        &self,
        candidate: &Invention,
    ) -> Result<HashMap<String, serde_json::Value>, RpcError> {
        let mut probe = candidate.clone();
        probe.developments.clear();
        let builder =
            InventionBuilder::with_registry(self.sample_rate, self.registry.for_validation());
        let (built, _) = builder
            .build(probe)
            .map_err(|error| build_failed("does not build", error.to_string()))?;
        let resolved = built
            .state
            .lock()
            .unwrap()
            .modules
            .iter()
            .map(|(id, info)| (id.clone(), info.config.clone()))
            .collect();
        Ok(resolved)
    }
}
