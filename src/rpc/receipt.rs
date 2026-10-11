//! Compact mutation acknowledgements: what a client that does not need the
//! whole graph back asks for instead of a full snapshot.

use crate::{ControlValue, RuntimeStatus};
use serde::{Deserialize, Serialize};

/// How a mutating command's acknowledgement is shaped, named by the request
/// (see [`RpcRequest::reply`]).
///
/// The default is [`MutationReply::Snapshot`], which every client that
/// predates receipts expects. A daemon that does not know the field ignores it
/// and answers with a snapshot, so a client asking for a receipt must still
/// accept one.
///
/// [`RpcRequest::reply`]: super::RpcRequest::reply
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MutationReply {
    /// The full runtime snapshot after the mutation: every module, its ports
    /// and controls, and every connection. Its size follows the invention.
    #[default]
    Snapshot,
    /// A [`MutationReceipt`]: status plus the controls the command wrote. Its
    /// size follows the request, not the invention, and the daemon builds no
    /// snapshot to answer it. Read the graph with `GetSnapshot` when needed.
    Receipt,
}

impl MutationReply {
    /// Whether this is the default, so the request can leave it off the wire.
    pub fn is_snapshot(&self) -> bool {
        matches!(self, Self::Snapshot)
    }
}

/// The compact acknowledgement of a mutation that committed, sent in place of
/// a snapshot when the request asked for [`MutationReply::Receipt`].
///
/// The envelope's `revision` carries the session and revision the mutation
/// produced; a refusal is still an `Error` payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct MutationReceipt {
    /// Running state, module and connection counts, and audio diagnostics
    /// when a device is attached.
    pub status: RuntimeStatus,
    /// Each control the command wrote, read back after the write so any
    /// coercion is visible. Empty for a command that writes no control.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<AppliedControl>,
}

/// A control as it reads after a write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct AppliedControl {
    pub module_id: String,
    pub key: String,
    /// The value as applied. `None` when the control could not be read back,
    /// for example because a later write in the same command removed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ControlValue>,
}
