//! Atomic structural edits: the `ApplyEdits` contract.
//!
//! [`RpcCommand::ApplyEdits`] carries an ordered batch of
//! [`StructuralEdit`]s. The daemon applies the batch to a copy of the
//! retained authored document, validates the result, and then commits it
//! all at once or not at all. A refusal changes nothing: no module, no
//! connection, no control, no revision, no event and no saved session.
//!
//! This module holds the wire types, the batch limits, the per-edit refusal
//! detail and the success report. The rules a host applies around the
//! command live with the other rules of their kind:
//!
//! - [`RpcCommand::advances_revision_after`]: the revision advances only on
//!   commit.
//! - [`RpcRequest::check_ticket`]: the command is refused without a
//!   [`MutationTicket`].
//! - [`RpcCommand::replay_policy`]: a lost reply is resent with its ticket.
//!
//! See `docs/apply-edits.md` for the full contract, written for adapter
//! authors.
//!
//! [`RpcCommand::ApplyEdits`]: super::RpcCommand::ApplyEdits
//! [`RpcCommand::advances_revision_after`]: super::RpcCommand::advances_revision_after
//! [`RpcRequest::check_ticket`]: super::RpcRequest::check_ticket
//! [`RpcCommand::replay_policy`]: super::RpcCommand::replay_policy
//! [`MutationTicket`]: super::MutationTicket

use serde::{Deserialize, Serialize};

use super::{RpcError, RpcErrorCode};
use crate::ControlValue;

/// The most edits one `ApplyEdits` batch may carry. An empty batch is refused
/// too, so a batch holds `1..=MAX_EDITS_PER_BATCH` edits.
pub const MAX_EDITS_PER_BATCH: usize = 256;

/// The longest module id, module type, port name or control key an edit may
/// name, in UTF-8 bytes. Every such name must also be non-empty.
pub const MAX_EDIT_NAME_BYTES: usize = 256;

/// One structural change in an `ApplyEdits` batch, tagged by `op`.
///
/// Edits apply in order against the candidate document, so a later edit sees
/// the effect of every earlier one: a module added at index 0 can be wired at
/// index 1. Unknown fields are refused, so a misspelled field (or an `intent`
/// on `set_control`) fails loudly instead of being ignored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuralEdit {
    /// Adds a new module. The id must not name a module that exists at this
    /// point in the batch; replacing a module is a `remove_module` followed by
    /// an `add_module` of the same id.
    AddModule {
        id: String,
        module_type: String,
        /// The module's authored configuration, exactly as a document would
        /// hold it. Omitted means `null`, the type's defaults.
        #[serde(default)]
        config: serde_json::Value,
    },
    /// Removes a module, together with every connection to or from it.
    RemoveModule { id: String },
    /// Connects an output port to an input port. Both ports must exist and
    /// the connection must not already exist.
    Connect {
        from: String,
        from_port: String,
        to: String,
        to_port: String,
    },
    /// Removes an existing connection.
    Disconnect {
        from: String,
        from_port: String,
        to: String,
        to_port: String,
    },
    /// Sets a control's authored starting value. Always authoring: the value
    /// is coerced to the control's declared kind and written into the
    /// module's configuration, as a standalone authored `SetControl` does.
    SetControl {
        module_id: String,
        key: String,
        value: ControlValue,
    },
}

/// The kind of a [`StructuralEdit`], as named on the wire by its `op` tag.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    AddModule,
    RemoveModule,
    Connect,
    Disconnect,
    SetControl,
}

impl EditOp {
    /// The wire name of the op.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AddModule => "add_module",
            Self::RemoveModule => "remove_module",
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
            Self::SetControl => "set_control",
        }
    }
}

impl std::fmt::Display for EditOp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl StructuralEdit {
    /// The kind of this edit.
    pub fn op(&self) -> EditOp {
        match self {
            Self::AddModule { .. } => EditOp::AddModule,
            Self::RemoveModule { .. } => EditOp::RemoveModule,
            Self::Connect { .. } => EditOp::Connect,
            Self::Disconnect { .. } => EditOp::Disconnect,
            Self::SetControl { .. } => EditOp::SetControl,
        }
    }

    /// Checks that every name the edit carries is non-empty and at most
    /// [`MAX_EDIT_NAME_BYTES`] long. `index` is the edit's position in its
    /// batch, reported in the refusal.
    pub fn check_names(&self, index: usize) -> Result<(), EditFailure> {
        let names: Vec<(&str, &str)> = match self {
            Self::AddModule {
                id, module_type, ..
            } => vec![("id", id), ("module_type", module_type)],
            Self::RemoveModule { id } => vec![("id", id)],
            Self::Connect {
                from,
                from_port,
                to,
                to_port,
            }
            | Self::Disconnect {
                from,
                from_port,
                to,
                to_port,
            } => vec![
                ("from", from),
                ("from_port", from_port),
                ("to", to),
                ("to_port", to_port),
            ],
            Self::SetControl { module_id, key, .. } => vec![("module_id", module_id), ("key", key)],
        };
        for (field, name) in names {
            if name.is_empty() || name.len() > MAX_EDIT_NAME_BYTES {
                return Err(EditFailure::new(
                    index,
                    self.op(),
                    EditFailureReason::InvalidName,
                    format!("`{field}` must be 1 to {MAX_EDIT_NAME_BYTES} bytes"),
                ));
            }
        }
        Ok(())
    }
}

/// Refuses a batch whose size is outside `1..=MAX_EDITS_PER_BATCH`.
///
/// A batch-level refusal: [`RpcErrorCode::InvalidRequest`] with no `edit`
/// detail, since no single edit is at fault.
pub fn check_edit_batch(edits: &[StructuralEdit]) -> Result<(), RpcError> {
    if edits.is_empty() {
        return Err(RpcError::new(
            RpcErrorCode::InvalidRequest,
            "apply_edits needs at least one edit",
        ));
    }
    if edits.len() > MAX_EDITS_PER_BATCH {
        return Err(RpcError::new(
            RpcErrorCode::InvalidRequest,
            format!(
                "apply_edits accepts at most {MAX_EDITS_PER_BATCH} edits per batch, got {}; \
                 split the change into smaller batches",
                edits.len()
            ),
        ));
    }
    Ok(())
}

/// Why one edit in a batch was refused. Machine-readable, so an adapter can
/// act on the cause without parsing the message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EditFailureReason {
    /// A module id, type, port name or control key is empty or longer than
    /// [`MAX_EDIT_NAME_BYTES`].
    InvalidName,
    /// The edit names a module that does not exist at this point in the
    /// batch (never existed, or an earlier edit removed it).
    UnknownModule,
    /// `add_module` names an id that already exists at this point in the
    /// batch.
    DuplicateModule,
    /// `add_module` names a type the running invention cannot build.
    UnknownModuleType,
    /// `add_module`'s config was refused by the module type.
    InvalidConfig,
    /// `connect` names an output or input port the module does not have.
    UnknownPort,
    /// `connect` names a connection that already exists.
    ConnectionExists,
    /// `disconnect` names a connection that does not exist.
    ConnectionNotFound,
    /// `set_control` names a key the module does not expose as a control.
    UnknownControl,
    /// `set_control`'s value cannot be coerced to the control's declared kind.
    InvalidControlValue,
}

/// The structured detail of a refused edit, carried on
/// [`RpcError::edit`](super::RpcError::edit) with
/// [`RpcErrorCode::InvalidEdit`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct EditFailure {
    /// Zero-based position of the failing edit in the batch.
    pub index: usize,
    /// The failing edit's op.
    pub op: EditOp,
    pub reason: EditFailureReason,
    /// What was wrong, for a human or an agent.
    pub message: String,
}

impl EditFailure {
    pub fn new(
        index: usize,
        op: EditOp,
        reason: EditFailureReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            index,
            op,
            reason,
            message: message.into(),
        }
    }
}

/// What a committed `ApplyEdits` batch did. Its size depends on the batch,
/// never on the size of the invention: every list holds at most one entry per
/// edit.
///
/// The response envelope's `revision` is the revision the batch committed at.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ApplyEditsReport {
    /// How many edits the batch carried; all of them were applied.
    pub edit_count: usize,
    /// Module ids added to the running graph.
    pub added: Vec<String>,
    /// Module ids removed from the running graph.
    pub removed: Vec<String>,
    /// Module ids whose instance was rebuilt (removed and added again in the
    /// same batch with a different type or config); their state restarts.
    pub rebuilt: Vec<String>,
    /// `module.key` for each distinct control the batch's `set_control` edits
    /// wrote, in first-written order.
    pub controls_written: Vec<String>,
    pub connections_added: usize,
    pub connections_removed: usize,
    /// Modules the batch left untouched; they keep their phase and state.
    pub untouched: usize,
}
