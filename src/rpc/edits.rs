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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{RpcError, RpcErrorCode};
use crate::invention::layout::Position;
use crate::invention::names::name_refusal;
use crate::ControlValue;

/// The most edits one `ApplyEdits` batch may carry. An empty batch is refused
/// too, so a batch holds `1..=MAX_EDITS_PER_BATCH` edits.
pub const MAX_EDITS_PER_BATCH: usize = 256;

/// The longest module id, module type, port name or control key an edit may
/// name, in UTF-8 bytes. Every such name must also be non-empty.
pub const MAX_EDIT_NAME_BYTES: usize = 256;

/// The most modules one `move_modules` edit may place. A `move_modules`
/// places at least one, so it holds `1..=MAX_MOVES_PER_EDIT` positions.
pub const MAX_MOVES_PER_EDIT: usize = 1024;

/// The most modules one `name_modules` edit may name. A `name_modules`
/// names at least one, so it holds `1..=MAX_NAMES_PER_EDIT` names.
pub const MAX_NAMES_PER_EDIT: usize = 1024;

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
        /// Its authored config, as a document holds it; omitted is `null`,
        /// the type's defaults.
        #[serde(default)]
        config: serde_json::Value,
        // What the piece calls the module, as `name_modules` sets it. A
        // plain comment: the agents' tool listing has no bytes to spare.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
        #[serde(deserialize_with = "crate::traits::exact_number::deserialize")]
        value: ControlValue,
    },
    /// Places modules on the canvas, in canvas units (`null` unplaces).
    /// Document-only: nothing is rebuilt or heard.
    // Positions are absolute, so the edit means the same however often it
    // is made. Every id must name a module that exists at this point in
    // the batch (see docs/apply-edits.md).
    MoveModules {
        positions: BTreeMap<String, Option<Position>>,
    },
    /// Names modules (`null` clears). Document-only; ids still address.
    NameModules {
        names: BTreeMap<String, Option<String>>,
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
    MoveModules,
    NameModules,
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
            Self::MoveModules => "move_modules",
            Self::NameModules => "name_modules",
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
            Self::MoveModules { .. } => EditOp::MoveModules,
            Self::NameModules { .. } => EditOp::NameModules,
        }
    }

    /// Checks that every name the edit carries is non-empty and at most
    /// [`MAX_EDIT_NAME_BYTES`] long. `index` is the edit's position in its
    /// batch, reported in the refusal.
    pub fn check_names(&self, index: usize) -> Result<(), EditFailure> {
        let check = |field: &str, name: &str| {
            if name.is_empty() || name.len() > MAX_EDIT_NAME_BYTES {
                return Err(EditFailure::new(
                    index,
                    self.op(),
                    EditFailureReason::InvalidName,
                    format!("`{field}` must be 1 to {MAX_EDIT_NAME_BYTES} bytes"),
                ));
            }
            Ok(())
        };
        match self {
            Self::AddModule {
                id, module_type, ..
            } => {
                check("id", id)?;
                check("module_type", module_type)
            }
            Self::RemoveModule { id } => check("id", id),
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
            } => {
                check("from", from)?;
                check("from_port", from_port)?;
                check("to", to)?;
                check("to_port", to_port)
            }
            Self::SetControl { module_id, key, .. } => {
                check("module_id", module_id)?;
                check("key", key)
            }
            Self::MoveModules { positions } => {
                positions.keys().try_for_each(|id| check("positions", id))
            }
            Self::NameModules { names } => names.keys().try_for_each(|id| check("names", id)),
        }
    }
}

/// Refuses a batch whose size is outside `1..=MAX_EDITS_PER_BATCH`, then a
/// `set_control` whose value is a non-finite number, then a `move_modules`
/// that places no module, more than [`MAX_MOVES_PER_EDIT`], or one at a
/// non-finite position ([`EditFailureReason::InvalidPosition`]), then a
/// `name_modules` that names no module, more than [`MAX_NAMES_PER_EDIT`],
/// or one by a name that cannot name it, or an `add_module` with such a
/// name ([`EditFailureReason::InvalidName`]).
///
/// A size refusal is batch-level: [`RpcErrorCode::InvalidRequest`] with no
/// `edit` detail, since no single edit is at fault. A non-finite number (NaN,
/// an infinity, or a value too large for an `f32`) is refused at its edit's
/// index as [`EditFailureReason::InvalidControlValue`], the refusal the daemon
/// gives when a JSON client sends one. JSON has no such numbers, so a client
/// that sends one without this check gets a `null` on the wire and a parse
/// error with no edit index.
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
    for (index, edit) in edits.iter().enumerate() {
        match edit {
            StructuralEdit::SetControl {
                module_id,
                key,
                value,
            } => {
                if let Some(message) = non_finite_refusal(module_id, key, value) {
                    return Err(RpcError::invalid_edit(EditFailure::new(
                        index,
                        EditOp::SetControl,
                        EditFailureReason::InvalidControlValue,
                        message,
                    )));
                }
            }
            StructuralEdit::MoveModules { positions } => {
                if let Some(message) = move_refusal(positions) {
                    return Err(RpcError::invalid_edit(EditFailure::new(
                        index,
                        EditOp::MoveModules,
                        EditFailureReason::InvalidPosition,
                        message,
                    )));
                }
            }
            StructuralEdit::NameModules { names } => {
                if let Some(message) = names_refusal(names) {
                    return Err(RpcError::invalid_edit(EditFailure::new(
                        index,
                        EditOp::NameModules,
                        EditFailureReason::InvalidName,
                        message,
                    )));
                }
            }
            StructuralEdit::AddModule {
                id,
                name: Some(name),
                ..
            } => {
                if let Some(reason) = name_refusal(name) {
                    return Err(RpcError::invalid_edit(EditFailure::new(
                        index,
                        EditOp::AddModule,
                        EditFailureReason::InvalidName,
                        format!("module '{id}': {reason}"),
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Why a `name_modules` edit's names cannot apply, if they cannot.
fn names_refusal(names: &BTreeMap<String, Option<String>>) -> Option<String> {
    if names.is_empty() {
        return Some("name_modules needs at least one name".to_string());
    }
    if names.len() > MAX_NAMES_PER_EDIT {
        return Some(format!(
            "name_modules names at most {MAX_NAMES_PER_EDIT} modules, got {}",
            names.len()
        ));
    }
    names.iter().find_map(|(id, name)| {
        let reason = name_refusal(name.as_deref()?)?;
        Some(format!("module '{id}': {reason}"))
    })
}

/// Why a `move_modules` edit's positions cannot apply, if they cannot.
fn move_refusal(positions: &BTreeMap<String, Option<Position>>) -> Option<String> {
    if positions.is_empty() {
        return Some("move_modules needs at least one position".to_string());
    }
    if positions.len() > MAX_MOVES_PER_EDIT {
        return Some(format!(
            "move_modules places at most {MAX_MOVES_PER_EDIT} modules, got {}",
            positions.len()
        ));
    }
    let (id, _) = (positions.iter())
        .find(|(_, position)| position.is_some_and(|position| !position.is_finite()))?;
    Some(format!("module '{id}' must be placed at a finite position"))
}

/// Why `value` cannot be written to `module_id.key`, if it is a number a
/// document cannot hold: NaN or an infinity, which is also what a number too
/// large for an `f32` becomes. JSON has no such numbers; a document would
/// hold `null`.
pub(crate) fn non_finite_refusal(
    module_id: &str,
    key: &str,
    value: &ControlValue,
) -> Option<String> {
    match value {
        ControlValue::Number(number) if !number.is_finite() => Some(format!(
            "control '{module_id}.{key}' expects a finite number, got {number}"
        )),
        _ => None,
    }
}

/// Why one edit in a batch was refused. Machine-readable, so an adapter can
/// act on the cause without parsing the message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EditFailureReason {
    /// A module id, type, port name or control key is empty or longer than
    /// [`MAX_EDIT_NAME_BYTES`]; or a module name is blank, too long, or
    /// holds a control character, or `name_modules` names none or too many.
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
    /// `set_control`'s value is refused: it cannot be coerced to the
    /// control's declared kind, it is not a finite number, or the module
    /// refuses it as its own setter would (a read-only control, unparsable
    /// JSON, an option the control does not offer).
    InvalidControlValue,
    /// `move_modules` places no module, too many, or one at a position that
    /// is not finite.
    InvalidPosition,
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
/// A field a newer daemon adds reads as its default from an older one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ApplyEditsReport {
    /// How many edits the batch carried; all of them were applied.
    pub edit_count: usize,
    /// Module ids added to the running graph.
    pub added: Vec<String>,
    /// Module ids removed from the running graph.
    pub removed: Vec<String>,
    /// Module ids that existed before the batch and were removed and added
    /// again in it. Each gets a fresh instance, even when its type and config
    /// are identical, so its state restarts.
    pub rebuilt: Vec<String>,
    /// Each distinct control the batch's `set_control` edits wrote, once, in
    /// first-written order. Only controls of modules that exist after the
    /// commit are listed: a write to a module a later edit removed or
    /// replaced is dropped with that module.
    pub controls_written: Vec<WrittenControl>,
    /// Control writes that passed validation but still failed when the
    /// commit wrote them (a sample that did not load, say). The batch is
    /// committed all the same: the module keeps the value it had, and the
    /// retained document records that value. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls_failed: Vec<ControlWriteFailure>,
    pub connections_added: usize,
    pub connections_removed: usize,
    /// Modules the batch left untouched; they keep their phase and state.
    /// A control-only change counts as untouched: the value changes, the
    /// instance does not.
    pub untouched: usize,
}

/// One control a committed batch wrote, named structurally because a module
/// id may itself contain a `.`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct WrittenControl {
    pub module_id: String,
    pub key: String,
}

/// A control write a committed batch could not make; see
/// [`ApplyEditsReport::controls_failed`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ControlWriteFailure {
    /// The `set_control` edit whose value failed: the last edit in the batch
    /// that wrote this control.
    pub edit_index: usize,
    pub module_id: String,
    pub key: String,
    /// The module's reason, cut to at most 256 bytes on a UTF-8 character
    /// boundary.
    pub error: String,
}

impl WrittenControl {
    pub fn new(module_id: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            module_id: module_id.into(),
            key: key.into(),
        }
    }
}
