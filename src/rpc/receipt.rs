//! Compact mutation acknowledgements: what a client that does not need the
//! whole graph back asks for instead of a full snapshot.

use crate::{ControlValue, RuntimeStatus};
use serde::{Deserialize, Serialize};

/// How a mutating command's acknowledgement is shaped, named by the request
/// (see [`RpcRequest::reply`]).
///
/// The default is [`MutationReply::Snapshot`], which every client that
/// predates receipts expects. A daemon that predates the field ignores it on a
/// command answered with a snapshot (see
/// [`RpcCommand::replies_with_snapshot`]) and sends one, so a client asking for
/// a receipt must still accept a snapshot. Send it on no other request: an
/// older daemon refuses the unknown field on commands whose payload denies
/// unknown fields, such as `DescribeModuleTypes`.
///
/// [`RpcCommand::replies_with_snapshot`]: super::RpcCommand::replies_with_snapshot
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
    /// A shape this build does not know, named by a newer client. Answered as
    /// [`MutationReply::Snapshot`], so a new shape can be added without
    /// failing requests to older daemons.
    #[serde(other)]
    Unknown,
}

impl MutationReply {
    /// Whether this is answered with a snapshot, so the request can leave it
    /// off the wire. True for [`MutationReply::Unknown`] too, which is never
    /// sent: it reads back as the default it is answered as.
    pub fn is_snapshot(&self) -> bool {
        matches!(self, Self::Snapshot | Self::Unknown)
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
    /// Each control the command wrote, in request order, read back once the
    /// command finished so any coercion is visible. A batch that writes one
    /// control twice reports its final value both times. Empty for a command
    /// that writes no control.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<AppliedControl>,
}

/// A control as it reads after a write, and what became of the write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct AppliedControl {
    pub module_id: String,
    pub key: String,
    /// The value as applied. `None` when the control could not be read back,
    /// for example because its module is no longer running, and when the
    /// write did not apply (see `state`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ControlValue>,
    /// The request the write was submitted as, which a `ControlChanged`
    /// event names once it applies. `None` for a write that applied as it
    /// was made, which no request reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// What became of the write by the time the reply was sent. `None`
    /// from a daemon that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ControlRequestState>,
    /// The value written, coerced to the control's kind: what plays once a
    /// pending write applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<ControlValue>,
    /// Why a refused write was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AppliedControl {
    /// A control read back after a write that reports nothing else.
    pub fn read_back(
        module_id: impl Into<String>,
        key: impl Into<String>,
        value: Option<ControlValue>,
    ) -> Self {
        Self {
            module_id: module_id.into(),
            key: key.into(),
            value,
            request: None,
            state: None,
            requested: None,
            reason: None,
        }
    }
}

/// What became of a control write when its reply was sent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlRequestState {
    /// It applied; `value` reads what plays.
    Applied,
    /// Submitted and not yet applied when the reply was sent: no device
    /// block took it in time, or none is running. A `ControlChanged` event
    /// naming its request follows if it applies; none follows if it is
    /// then superseded or overridden.
    Pending,
    /// A later write to the control at the same sample replaced it.
    Superseded,
    /// A higher-ranked write held the control.
    Overridden,
    /// Refused as it was applied; `reason` says why.
    Refused,
}

impl super::RpcCommand {
    /// Whether a daemon acknowledges this command with a full snapshot unless
    /// the request asks for a [`MutationReceipt`]: the commands that load,
    /// reload or unload an invention, write controls, or edit its graph. Only
    /// these honor [`RpcRequest::reply`], so a client sends a non-default
    /// reply on no other command.
    ///
    /// [`RpcRequest::reply`]: super::RpcRequest::reply
    pub fn replies_with_snapshot(&self) -> bool {
        // Exhaustive, so a new command needs a decision here.
        match self {
            // A timed edit replies with the request's status.
            Self::ReloadInvention { timing, .. } | Self::SwapModule { timing, .. } => {
                !timing.is_timed()
            }
            Self::LoadInvention { .. }
            | Self::LoadExample { .. }
            | Self::UnloadInvention
            | Self::SetControl { .. }
            | Self::SetControls { .. }
            | Self::AddModule { .. }
            | Self::RemoveModule { .. }
            | Self::Connect { .. }
            | Self::Disconnect { .. } => true,
            Self::ApplyEdits { .. }
            | Self::InspectInvention { .. }
            | Self::GetInvention { .. }
            | Self::SaveInvention { .. }
            | Self::InstallPackage(_)
            | Self::ListPackages { .. }
            | Self::ListDevelopments { .. }
            | Self::DescribeDevelopment { .. }
            | Self::ListExamples { .. }
            | Self::DescribeExample { .. }
            | Self::ExportRequestLog { .. }
            | Self::DescribeModuleTypes(_)
            | Self::DescribeModule(_)
            | Self::FlushSession { .. }
            | Self::GetRequest { .. }
            | Self::ListPendingRequests { .. }
            | Self::Shutdown => false,
        }
    }

    /// The controls a receipt for this command reads back, as
    /// `(module_id, key)` in request order: each write of a `SetControl` or
    /// `SetControls`. Empty for every other command.
    pub fn written_controls(&self) -> Vec<(&str, &str)> {
        match self {
            Self::SetControl { module_id, key, .. } => vec![(module_id, key)],
            Self::SetControls { writes } => writes
                .iter()
                .map(|write| (write.module_id.as_str(), write.key.as_str()))
                .collect(),
            _ => Vec::new(),
        }
    }
}
