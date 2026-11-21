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
//! An offline render keeps no authored document, so the batch refuses the
//! document operations (`set_control`, `move_modules`, `name_modules`): a
//! host that keeps the document writes those itself.

use super::RenderEngine;
use crate::invention::publish::{edge, BuiltModule, GraphChange};
use crate::invention::runtime::GraphCommandError;
use crate::rpc::{
    check_edit_batch, EditFailure, EditFailureReason, EditOp, RpcError, RpcErrorCode,
    StructuralEdit,
};

impl RenderEngine {
    /// Applies `edits` to the loaded graph as one change, in order, or
    /// refuses them all, naming the first edit that cannot apply. Returns
    /// how many edits applied.
    ///
    /// The checks are those of `ApplyEdits`: an `add_module` id must not
    /// name a module that exists at that point in the batch, a
    /// `remove_module` id must name one, a `connect` must name existing
    /// ports and a connection not yet made, a `disconnect` one that is.
    pub fn apply_edits(&self, edits: &[StructuralEdit]) -> Result<usize, RpcError> {
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
        let graph = self.loaded().map_err(graph_refusal)?;
        let mut built = self.build_added(edits)?;
        let mut failure = None;
        let committed = self.edit(graph, |change| {
            for (index, edit) in edits.iter().enumerate() {
                if let Err(refused) = apply_one(change, edit, built[index].take()) {
                    let message = refused.to_string();
                    failure = Some(EditFailure::new(
                        index,
                        edit.op(),
                        reason(&refused),
                        message,
                    ));
                    return Err(refused.into());
                }
            }
            Ok(())
        });
        match (committed, failure) {
            (Ok(committed), _) => {
                self.follow_up(committed);
                Ok(edits.len())
            }
            (Err(_), Some(failure)) => Err(RpcError::invalid_edit(failure)),
            (Err(error), None) => Err(graph_refusal(error)),
        }
    }

    /// Builds each added module from its config, in batch order, before
    /// anything is touched; a module that does not build refuses the batch.
    fn build_added(&self, edits: &[StructuralEdit]) -> Result<Vec<Option<BuiltModule>>, RpcError> {
        let mut built = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            let StructuralEdit::AddModule {
                id,
                module_type,
                config,
                ..
            } = edit
            else {
                built.push(None);
                continue;
            };
            let module =
                GraphChange::build(&self.registry, self.sample_rate, id, module_type, config)
                    .map_err(|error| {
                        let reason = match error {
                            GraphCommandError::UnknownModuleType(_) => {
                                EditFailureReason::UnknownModuleType
                            }
                            _ => EditFailureReason::InvalidConfig,
                        };
                        RpcError::invalid_edit(EditFailure::new(
                            index,
                            EditOp::AddModule,
                            reason,
                            error.to_string(),
                        ))
                    })?;
            built.push(Some(module));
        }
        Ok(built)
    }
}

/// Makes `edit` in `change`, with `built`, the module an `add_module` built;
/// the change is dropped whole when any edit fails.
fn apply_one(
    change: &mut GraphChange,
    edit: &StructuralEdit,
    built: Option<BuiltModule>,
) -> Result<(), Refused> {
    match edit {
        StructuralEdit::AddModule { id, .. } => {
            if change.contains(id) {
                return Err(Refused::Duplicate(id.clone()));
            }
            change.upsert(
                id,
                built.expect("each added module is built before the change"),
            );
        }
        StructuralEdit::RemoveModule { id } => {
            if !change.contains(id) {
                return Err(Refused::Graph(GraphCommandError::UnknownModule(id.clone())));
            }
            change.remove(id);
        }
        StructuralEdit::Connect {
            from,
            from_port,
            to,
            to_port,
        } => {
            let wire = edge(from, from_port, to, to_port);
            if change.has_edge(&wire) {
                return Err(Refused::Exists(format!(
                    "{from}.{from_port} -> {to}.{to_port}"
                )));
            }
            change.connect(wire).map_err(Refused::Graph)?;
        }
        StructuralEdit::Disconnect {
            from,
            from_port,
            to,
            to_port,
        } => {
            let wire = edge(from, from_port, to, to_port);
            if !change.has_edge(&wire) {
                return Err(Refused::Missing(format!(
                    "{from}.{from_port} -> {to}.{to_port}"
                )));
            }
            change.disconnect(wire);
        }
        // Refused before the change began.
        _ => unreachable!("document edits never reach the change"),
    }
    Ok(())
}

/// Why one edit of a batch cannot apply.
#[derive(Debug)]
enum Refused {
    Duplicate(String),
    Exists(String),
    Missing(String),
    Graph(GraphCommandError),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(id) => write!(
                f,
                "module '{id}' already exists; remove_module it first to replace it"
            ),
            Self::Exists(wire) => write!(f, "connection {wire} already exists"),
            Self::Missing(wire) => write!(f, "there is no connection {wire}"),
            Self::Graph(error) => write!(f, "{error}"),
        }
    }
}

impl From<Refused> for GraphCommandError {
    fn from(refused: Refused) -> Self {
        match refused {
            Refused::Graph(error) => error,
            other => GraphCommandError::ControlError(other.to_string()),
        }
    }
}

/// A refusal no single edit caused, as `ApplyEdits` words one: a schedule
/// that still targets a removed module, say, or nothing loaded.
fn graph_refusal(error: GraphCommandError) -> RpcError {
    match error {
        GraphCommandError::AudioThreadStopped => RpcError::from(error),
        other => RpcError::new(
            RpcErrorCode::ModuleBuildFailed,
            format!("the edited invention could not be prepared: {other}; nothing was applied"),
        ),
    }
}

fn reason(refused: &Refused) -> EditFailureReason {
    match refused {
        Refused::Duplicate(_) => EditFailureReason::DuplicateModule,
        Refused::Exists(_) => EditFailureReason::ConnectionExists,
        Refused::Missing(_) => EditFailureReason::ConnectionNotFound,
        Refused::Graph(GraphCommandError::UnknownModule(_)) => EditFailureReason::UnknownModule,
        Refused::Graph(_) => EditFailureReason::UnknownPort,
    }
}
