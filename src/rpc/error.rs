//! Structured RPC errors.

use super::{EditFailure, RevisionConflict};
use crate::GraphCommandError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
    /// Present only on [`RpcErrorCode::RevisionConflict`]: the compact
    /// structured body a client needs to re-read and rebase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<RevisionConflict>,
    /// Present only on [`RpcErrorCode::InvalidEdit`]: which edit of an
    /// `ApplyEdits` batch was refused, and why. Boxed to keep `RpcError`
    /// small on the `Ok` path of every fallible RPC call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<Box<EditFailure>>,
}

impl RpcError {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            conflict: None,
            edit: None,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Unsupported, message)
    }

    /// Builds the refusal for an unmet revision precondition. The caller must
    /// have mutated nothing.
    pub fn revision_conflict(conflict: RevisionConflict) -> Self {
        Self {
            code: RpcErrorCode::RevisionConflict,
            message: conflict.describe(),
            conflict: Some(conflict),
            edit: None,
        }
    }

    /// Builds the refusal for one edit of an `ApplyEdits` batch. The caller
    /// must have changed nothing: the whole batch is refused.
    pub fn invalid_edit(failure: EditFailure) -> Self {
        Self {
            code: RpcErrorCode::InvalidEdit,
            message: format!(
                "edit {} ({}) refused: {}; nothing was applied",
                failure.index, failure.op, failure.message
            ),
            conflict: None,
            edit: Some(Box::new(failure)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    /// The discovery request contains an invalid selector or exceeds an input limit.
    InvalidRequest,
    /// The discovery response exceeds its documented size bound.
    ResponseTooLarge,
    IncompatibleSchemaVersion,
    /// The request's `expected_revision` did not match the daemon's current
    /// revision. Nothing was mutated; the error carries a
    /// [`RevisionConflict`] describing both sides.
    RevisionConflict,
    /// A retried ticketed command whose original is older than the daemon's
    /// recovery ledger remembers. Nothing ran; the original may or may not
    /// have applied, so re-read state before editing again.
    MutationExpired,
    /// One edit of an `ApplyEdits` batch was refused, so the whole batch was.
    /// Nothing changed; the error carries an [`EditFailure`] naming the edit
    /// and the reason.
    InvalidEdit,
    AudioThreadStopped,
    UnknownModuleType,
    ModuleBuildFailed,
    UnknownModule,
    InvalidPort,
    ControlError,
    Unsupported,
    Internal,
}

impl From<GraphCommandError> for RpcError {
    fn from(error: GraphCommandError) -> Self {
        let code = match error {
            GraphCommandError::AudioThreadStopped => RpcErrorCode::AudioThreadStopped,
            GraphCommandError::UnknownModuleType(_) => RpcErrorCode::UnknownModuleType,
            GraphCommandError::ModuleBuildFailed(_) => RpcErrorCode::ModuleBuildFailed,
            GraphCommandError::UnknownModule(_) => RpcErrorCode::UnknownModule,
            GraphCommandError::InvalidPort(_) => RpcErrorCode::InvalidPort,
            GraphCommandError::ControlError(_) => RpcErrorCode::ControlError,
            // Nothing was applied; the messages say why.
            GraphCommandError::QueueFull | GraphCommandError::TopologyMoved => {
                RpcErrorCode::Internal
            }
        };
        Self::new(code, error.to_string())
    }
}
