//! An offline render's batch with `set_control` edits, checked as the
//! daemon checks one.

use super::facts::{GraphFacts, LiveFacts};
use super::{check_writes, EditedCandidate};
use crate::invention::edits::apply_to_candidate;
use crate::invention::format::Invention;
use crate::rpc::{check_edit_batch, RpcError, StructuralEdit};

/// Applies `edits` to a copy of `document`, an offline render's retained
/// document, checked against `graph`, the render's modules and registry, as
/// the daemon checks a batch before it plans one: each edit in order on the
/// candidate, every control write made on a throwaway copy of its module,
/// and each control's final value against the directory the batch leaves.
/// Changes nothing; returns the candidate, or the refusal naming the first
/// edit that cannot apply.
///
/// Unlike the daemon's, the edited invention is not built whole here: the
/// render's own change checks its schedules as it commits.
pub(crate) fn check_offline(
    document: &Invention,
    edits: &[StructuralEdit],
    graph: GraphFacts,
) -> Result<EditedCandidate, RpcError> {
    check_edit_batch(edits)?;
    // An offline render takes its writes at once: none is left waiting.
    let mut facts = LiveFacts::of(graph, document, Vec::new());
    let candidate =
        apply_to_candidate(document, edits, &mut facts).map_err(RpcError::invalid_edit)?;
    check_writes(&facts.directory_after(&candidate.document), &candidate)?;
    Ok(candidate)
}
