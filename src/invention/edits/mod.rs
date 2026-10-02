//! Candidate documents for atomic edit batches.
//!
//! An `ApplyEdits` batch is first applied to a copy of the retained authored
//! document, never to the running graph. [`apply_to_candidate`] is that
//! step: a pure function from (document, runtime facts, edits) to the
//! candidate document, or to the first edit that cannot apply. Nothing about
//! the running invention changes here; validating, planning and publishing
//! the candidate come after.
//!
//! The facts the edits are checked against (which ports a module has, which
//! controls it exposes and their kinds) come through [`EditFacts`], so the
//! live runtime and a test fake plug in the same way. The candidate is
//! written with the same helpers the runtime uses for single edits
//! ([`authored_document`]), so a candidate saves identically to the document
//! the equivalent single commands would leave behind.
//!
//! See `docs/apply-edits.md` for the contract a client sees.
//!
//! [`authored_document`]: super::authored_document

mod candidate;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use crate::invention::format::Invention;
use crate::rpc::{ControlWrite, EditFailure, StructuralEdit};
use crate::{ControlKind, ControlSurface, GraphModule};

/// What a batch needs to know about one module: its ports and its controls.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModuleFacts {
    pub(crate) inputs: Vec<String>,
    pub(crate) outputs: Vec<String>,
    /// Control keys and the kind each value is coerced to.
    pub(crate) controls: BTreeMap<String, ControlKind>,
}

impl ModuleFacts {
    /// Reads the facts from a built module and its control surface.
    pub(crate) fn from_instance(
        module: &GraphModule,
        surface: Option<&dyn ControlSurface>,
    ) -> Self {
        let module = module.module();
        Self {
            inputs: module
                .inputs()
                .iter()
                .map(|port| port.to_string())
                .collect(),
            outputs: module
                .outputs()
                .iter()
                .map(|port| port.to_string())
                .collect(),
            controls: surface
                .map(|surface| {
                    surface
                        .controls()
                        .into_iter()
                        .map(|meta| (meta.key, meta.kind))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The runtime facts a batch is checked against.
///
/// Existence is decided by the candidate document; these facts answer what
/// a module that exists can do. The live runtime implements this over its
/// running modules and its current registry; tests use a fake.
pub(crate) trait EditFacts {
    /// Facts for a module in the document the batch started from, or `None`
    /// when no such module is running.
    fn module(&self, id: &str) -> Option<ModuleFacts>;

    /// Whether `module_type` can be built by the running invention.
    fn has_type(&self, module_type: &str) -> bool;

    /// Facts for a module `add_module` would build as `id` from this type
    /// and config, or the reason the type refuses the config. Only called
    /// for types [`Self::has_type`] accepts. An implementation may keep what
    /// it built for `id`, to reuse it when the batch commits.
    fn describe(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<ModuleFacts, String>;
}

/// One `set_control` edit's authored write, tagged with the edit that made
/// it so a refusal at commit time can name that edit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CandidateWrite {
    /// The `set_control` edit's position in the batch.
    pub(crate) edit_index: usize,
    /// The write, carrying the value as coerced to the control's kind.
    pub(crate) write: ControlWrite,
}

/// A batch applied to a copy of the authored document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EditedCandidate {
    /// The authored document with every edit applied, in order.
    pub(crate) document: Invention,
    /// One authored write per `set_control` edit, in batch order. The commit
    /// checks each against the module's own rules before publishing, and
    /// refuses a write the module would refuse as `invalid_control_value`
    /// at its `edit_index`.
    pub(crate) control_writes: Vec<CandidateWrite>,
    /// Every module id an edit names, as a module or as a connection
    /// endpoint. A commit must not touch a module outside this set.
    pub(crate) named_modules: BTreeSet<String>,
}

/// Applies `edits` in order to a copy of `document`, the retained authored
/// document with its connections (see `RuntimeState::document`).
///
/// Returns the candidate, or the first edit that cannot apply, with its
/// index and reason. The batch size is the caller's to check first (see
/// [`check_edit_batch`](crate::rpc::check_edit_batch)).
pub(crate) fn apply_to_candidate(
    document: &Invention,
    edits: &[StructuralEdit],
    facts: &mut impl EditFacts,
) -> Result<EditedCandidate, EditFailure> {
    let mut candidate = candidate::Candidate::new(document.clone(), facts);
    for (index, edit) in edits.iter().enumerate() {
        edit.check_names(index)?;
        candidate
            .apply(index, edit)
            .map_err(|refusal| EditFailure::new(index, edit.op(), refusal.0, refusal.1))?;
    }
    Ok(candidate.finish())
}
