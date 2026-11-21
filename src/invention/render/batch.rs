//! A batch of structural edits on an offline render, applied as one change
//! or refused with nothing changed.
//!
//! The batch takes the wire form of `ApplyEdits` ([`StructuralEdit`]) and
//! its four graph operations: `add_module`, `remove_module`, `connect` and
//! `disconnect`. Edits apply in order, so a module added at index 0 can be
//! wired at index 1. Modules are built first, outside the graph's lock;
//! then every edit goes into one [`GraphChange`], which commits whole or
//! not at all (see [`edit_offline`](crate::invention::publish::edit_offline)).
//!
//! The checks and their refusals are the daemon's (see
//! [`refusals`](crate::invention::edits::refusals)), made in the same
//! order, so the same bad edit is refused with the same reason and message.
//! A refusal of the batch as a whole, a schedule it leaves unable to
//! resolve, has the daemon's code and names the same scheduler and reason;
//! one whose schedule targets a module the batch removes also says to
//! write the schedule in the same change.
//!
//! An offline render keeps no authored document, so the batch refuses the
//! document operations (`set_control`, `move_modules`, `name_modules`) and
//! ignores an `add_module`'s `name`: a host that keeps the document writes
//! those itself.

use super::RenderEngine;
use crate::invention::edits::refusals::{
    already_connected, duplicate_module, no_input_port, no_output_port, not_connected,
    refused_config, unknown_module, unknown_module_type, Refusal,
};
use crate::invention::edits::{build_failed, graph_refusal};
use crate::invention::publish::{edge, BuiltModule, GraphChange};
use crate::invention::runtime::GraphCommandError;
use crate::rpc::{
    check_edit_batch, EditFailure, EditFailureReason, EditOp, RpcError, RpcErrorCode,
    StructuralEdit,
};

/// What building an `add_module`'s module came to, kept until its edit's
/// turn so a refusal names the first edit that cannot apply.
type Build = Option<Result<BuiltModule, Refusal>>;

impl RenderEngine {
    /// Applies `edits` to the loaded graph as one change, in order, or
    /// refuses them all, naming the first edit that cannot apply.
    ///
    /// The checks are those of `ApplyEdits`: an `add_module` id must not
    /// name a module that exists at that point in the batch, and its type
    /// and config must build; a `remove_module` id must name one; a
    /// `connect` must name existing modules and ports and a connection not
    /// yet made, a `disconnect` existing modules and a connection that is.
    /// Such a refusal carries the edit's index, op and reason. A document
    /// operation is refused for the whole batch, with no edit detail, as an
    /// oversized batch is.
    pub fn apply_edits(&self, edits: &[StructuralEdit]) -> Result<(), RpcError> {
        check_edit_batch(edits)?;
        for (index, edit) in edits.iter().enumerate() {
            edit.check_names(index).map_err(RpcError::invalid_edit)?;
            if !matches!(
                edit.op(),
                EditOp::AddModule | EditOp::RemoveModule | EditOp::Connect | EditOp::Disconnect
            ) {
                return Err(RpcError::new(
                    RpcErrorCode::InvalidRequest,
                    format!(
                        "edit {index} ({}) refused: an offline render applies only add_module, \
                         remove_module, connect and disconnect; nothing was applied",
                        edit.op()
                    ),
                ));
            }
        }
        let graph = self.loaded().map_err(RpcError::from)?;
        let mut built = self.build_added(edits);
        let mut failure = None;
        let committed = self.edit(graph, |change| {
            for (index, edit) in edits.iter().enumerate() {
                if let Err(Refusal(reason, message)) = apply_one(change, edit, &mut built[index]) {
                    failure = Some(EditFailure::new(index, edit.op(), reason, message.clone()));
                    return Err(GraphCommandError::ControlError(message));
                }
            }
            Ok(())
        });
        // A module built for an edit that never got its turn drops here,
        // after the graph's lock.
        drop(built);
        match (committed, failure) {
            (Ok(committed), _) => {
                self.follow_up(committed);
                Ok(())
            }
            (Err(_), Some(failure)) => Err(RpcError::invalid_edit(failure)),
            // A schedule the batch leaves unable to resolve, as building
            // the edited invention refuses it in the daemon.
            (Err(GraphCommandError::ModuleBuildFailed(reason)), None) => {
                Err(build_failed("does not build", reason))
            }
            (Err(error), None) => Err(graph_refusal(error)),
        }
    }

    /// Builds each added module from its config, in batch order, before
    /// the graph is touched. A build that fails is kept, to refuse its edit
    /// in its turn.
    fn build_added(&self, edits: &[StructuralEdit]) -> Vec<Build> {
        edits
            .iter()
            .map(|edit| {
                let StructuralEdit::AddModule {
                    id,
                    module_type,
                    config,
                    ..
                } = edit
                else {
                    return None;
                };
                let built =
                    GraphChange::build(&self.registry, self.sample_rate, id, module_type, config);
                Some(built.map_err(|error| match error {
                    GraphCommandError::UnknownModuleType(_) => unknown_module_type(module_type),
                    GraphCommandError::ModuleBuildFailed(reason) => {
                        // The factory's reason, without the id
                        // `GraphChange::build` puts before it.
                        let prefix = format!("module '{id}': ");
                        let reason = reason.strip_prefix(&prefix).unwrap_or(&reason);
                        refused_config(id, module_type, reason)
                    }
                    other => refused_config(id, module_type, &other.to_string()),
                }))
            })
            .collect()
    }
}

/// Makes `edit` in `change`, with `built`, what building an `add_module`'s
/// module came to, checking in the order `ApplyEdits` does. The change is
/// dropped whole when any edit fails.
fn apply_one(
    change: &mut GraphChange,
    edit: &StructuralEdit,
    built: &mut Build,
) -> Result<(), Refusal> {
    match edit {
        StructuralEdit::AddModule { id, .. } => {
            if change.contains(id) {
                return Err(duplicate_module(id));
            }
            let module = built
                .take()
                .expect("each added module is built before the change")?;
            change.upsert(id, module);
        }
        StructuralEdit::RemoveModule { id } => {
            if !change.contains(id) {
                return Err(unknown_module(id));
            }
            change.remove(id);
        }
        StructuralEdit::Connect {
            from,
            from_port,
            to,
            to_port,
        } => {
            let source = change.ports(from).ok_or_else(|| unknown_module(from))?;
            if !source.outputs.contains(from_port) {
                return Err(no_output_port(from, from_port, &source.outputs));
            }
            let dest = change.ports(to).ok_or_else(|| unknown_module(to))?;
            if !dest.inputs.contains(to_port) {
                return Err(no_input_port(to, to_port, &dest.inputs));
            }
            let wire = edge(from, from_port, to, to_port);
            if change.has_edge(&wire) {
                return Err(already_connected(from, from_port, to, to_port));
            }
            // Both ends and their ports were checked above.
            change
                .connect(wire)
                .expect("a connection between checked ports");
        }
        StructuralEdit::Disconnect {
            from,
            from_port,
            to,
            to_port,
        } => {
            for id in [from, to] {
                if !change.contains(id) {
                    return Err(unknown_module(id));
                }
            }
            let wire = edge(from, from_port, to, to_port);
            if !change.has_edge(&wire) {
                return Err(not_connected(from, from_port, to, to_port));
            }
            change.disconnect(wire);
        }
        // Refused before the change began.
        _ => unreachable!("document edits never reach the change"),
    }
    Ok(())
}
