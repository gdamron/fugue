//! The `set_control` edits of a batch on an offline render: checked as the
//! daemon checks them, on a copy of the render's retained document, and
//! made once the batch's graph change commits.
//!
//! The render keeps the document it loaded, updated by each edit, as the
//! daemon keeps its retained document; it has no call to export it. So the
//! report gives each written module's config as that document now holds
//! it, which a host keeping its own copy of the document puts in place of
//! the module's config there.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::RenderEngine;
use crate::invention::audio_assets;
use crate::invention::edits::{check_offline, EditedCandidate, GraphFacts};
use crate::invention::publish::Contested;
use crate::invention::runtime::GraphCommandError;
use crate::rpc::{
    truncate_on_char_boundary, ControlWriteFailure, RpcError, RpcErrorCode, StructuralEdit,
    WrittenControl, MODULE_ERROR_BYTES,
};

/// What a committed batch on an offline render wrote: the control fields
/// of the daemon's `ApplyEditsReport`, by the same names and meanings, and
/// the configs a host needs to keep its own copy of the document.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderEditsReport {
    /// Each distinct control the batch's `set_control` edits wrote, once,
    /// in first-written order, for modules that exist after the batch.
    pub controls_written: Vec<WrittenControl>,
    /// Writes that passed every check but failed when made (a sample that
    /// did not load, say). The batch is committed all the same: the module
    /// keeps the value it had, and the document what it held. Omitted when
    /// empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls_failed: Vec<ControlWriteFailure>,
    /// The config of each module a `set_control` wrote, once, in
    /// first-written order, as the render's retained document now holds
    /// it: the written values are its starting values. Empty for a batch
    /// without a `set_control`.
    pub configs: Vec<AuthoredConfig>,
}

/// One module's config as the render's retained document holds it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AuthoredConfig {
    pub module_id: String,
    pub config: serde_json::Value,
}

impl RenderEngine {
    /// The batch checked as the daemon checks it, with its control writes,
    /// when it has a `set_control`; `None`, checking nothing more, when it
    /// has none. Changes nothing.
    pub(super) fn checked_writes(
        &self,
        edits: &[StructuralEdit],
    ) -> Result<Option<EditedCandidate>, RpcError> {
        if !(edits.iter()).any(|edit| matches!(edit, StructuralEdit::SetControl { .. })) {
            return Ok(None);
        }
        let (document, developments) = {
            let state = self.state.lock().unwrap();
            (state.document(), state.developments.loaded().clone())
        };
        let document = document.ok_or_else(|| {
            RpcError::new(
                RpcErrorCode::Unsupported,
                "the render keeps no authored document to edit; nothing was applied",
            )
        })?;
        let graph = GraphFacts {
            registry: Arc::new(self.registry.clone()),
            sample_rate: self.sample_rate,
            ports: self.module_ports.lock().unwrap().clone(),
            surfaces: self.control_surfaces.lock().unwrap().clone(),
            developments,
        };
        check_offline(&document, edits, graph).map(Some)
    }

    /// Makes the checked batch's writes once its graph change committed, as
    /// the daemon's commit does: the candidate becomes the retained
    /// document, and each module that survived the change is written, in
    /// batch order, unrecorded unless `contested` says a timed authored write
    /// contests it, as the daemon writes them. Modules the batch added or
    /// rebuilt were built with their writes. A write that fails as it is made is reported, and the
    /// document takes back what it held for the control.
    pub(super) fn commit_writes(
        &self,
        edits: &[StructuralEdit],
        candidate: EditedCandidate,
        contested: &Contested,
    ) -> RenderEditsReport {
        let built: HashSet<&str> = (edits.iter())
            .filter_map(|edit| match edit {
                StructuralEdit::AddModule { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let previous = {
            let mut state = self.state.lock().unwrap();
            for write in candidate.control_writes.iter().map(|c| &c.write) {
                if !built.contains(write.module_id.as_str()) {
                    state.write_stored_control(&write.module_id, &write.key, &write.value);
                }
            }
            state.document.replace(candidate.document.clone())
        };
        let snapshot = self.snapshot();
        let source_path = candidate.document.source_path.as_deref();
        // Each control is settled by its last write: an earlier failure a
        // later write made good is not a failure.
        let mut last: HashMap<(&str, &str), Option<String>> = HashMap::new();
        for written in &candidate.control_writes {
            let write = &written.write;
            if built.contains(write.module_id.as_str()) {
                continue;
            }
            let asset = (write.key.as_str(), written.asset);
            let load = audio_assets::resolve_checked_asset_write(source_path, asset, &write.value);
            let control = (write.module_id.as_str(), write.key.as_str());
            let made = snapshot.set_control_committed(control, load, contested);
            let outcome = made.err().map(|error| match error {
                GraphCommandError::ControlError(message) => message,
                other => other.to_string(),
            });
            last.insert((write.module_id.as_str(), write.key.as_str()), outcome);
        }
        let mut report = RenderEditsReport::default();
        let mut modules: Vec<&str> = Vec::new();
        for written in candidate.final_writes() {
            let write = &written.write;
            let control = (write.module_id.as_str(), write.key.as_str());
            if !modules.contains(&control.0) {
                modules.push(control.0);
            }
            let Some(Some(mut error)) = last.get(&control).cloned() else {
                let named = WrittenControl::new(&write.module_id, &write.key);
                report.controls_written.push(named);
                continue;
            };
            if let Ok(actual) = snapshot.get_control(control.0, control.1) {
                let mut state = self.state.lock().unwrap();
                state.restore_authored_control(previous.as_ref(), control, &actual);
            }
            truncate_on_char_boundary(&mut error, MODULE_ERROR_BYTES);
            report.controls_failed.push(ControlWriteFailure {
                edit_index: written.edit_index,
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                error,
            });
        }
        let state = self.state.lock().unwrap();
        let document = state.document.as_ref();
        for module_id in modules {
            let spec = document.and_then(|d| d.modules.iter().find(|spec| spec.id == module_id));
            if let Some(spec) = spec {
                report.configs.push(AuthoredConfig {
                    module_id: module_id.to_string(),
                    config: spec.config.clone(),
                });
            }
        }
        report
    }
}
