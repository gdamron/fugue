//! Control writes submitted and not yet applied, on the wire: what
//! [`RpcCommand::ListPendingRequests`](super::RpcCommand::ListPendingRequests)
//! lists.

use serde::{Deserialize, Serialize};

use super::RequestLanding;
use crate::ControlValue;

/// The most pending writes a compact listing names; the rest are counted.
pub const COMPACT_PENDING_ITEMS: usize = 8;

/// How much a pending listing says.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PendingDetail {
    /// The counts, and the oldest [`COMPACT_PENDING_ITEMS`] writes.
    #[default]
    Compact,
    /// The counts, and every write.
    Full,
}

/// Who submitted a pending write: a front door's user or agent, a script,
/// or automation (a schedule). A timed edit's writes, made as it lands,
/// are the user's, the rank an edit's writes have.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PendingSource {
    User,
    Agent,
    Script,
    Automation,
}

/// What a pending write waits for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PendingWait {
    /// The next block the audio thread plays.
    NextBlock,
    /// A sample or a moment to come.
    Time,
    /// A beat of a clock.
    Beat,
    /// A condition on the running invention, such as a timed edit landing.
    Condition,
}

/// A control write submitted and not yet applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PendingRequest {
    /// The request it was submitted as. A front door's write is announced
    /// by a `ControlChanged` event naming it once it applies; the copy of
    /// one kept over a timed edit is the edit's, and is not.
    pub request: String,
    pub module_id: String,
    pub key: String,
    /// The value written, coerced to the control's kind.
    pub requested: ControlValue,
    pub source: PendingSource,
    pub waiting: PendingWait,
    /// Where a timed write applies, as far as is known: its sample when
    /// timed to one, its beat when timed in beats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<RequestLanding>,
}

/// How many pending writes each source has.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PendingCounts {
    pub user: usize,
    pub agent: usize,
    pub script: usize,
    pub automation: usize,
}

/// The control writes submitted and not yet applied, oldest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PendingRequests {
    /// Every pending write the listing's `source` takes in.
    pub total: usize,
    /// Every pending write, whatever the listing's `source`, by source.
    pub by_source: PendingCounts,
    /// The writes listed: all of them in full, the oldest
    /// [`COMPACT_PENDING_ITEMS`] in a compact listing.
    pub items: Vec<PendingRequest>,
    /// How many more there are than `items` lists.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}
