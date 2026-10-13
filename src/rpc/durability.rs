//! How far the daemon's saved session lags its live state.
//!
//! A daemon applies a mutation and replies before the session reaches disk;
//! it saves in the background shortly after. [`SessionDurability`] reports
//! where that saving stands, on every runtime status the daemon sends and in
//! reply to [`RpcCommand::FlushSession`], which waits for disk.
//!
//! [`RpcCommand::FlushSession`]: super::RpcCommand::FlushSession

use super::RuntimeRevision;
use serde::{Deserialize, Serialize};

/// Where saving the daemon session stands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SessionSaveState {
    /// Every change the daemon has applied is on disk.
    Saved,
    /// Changes are applied but not yet on disk; see
    /// [`SessionDurability::behind_ms`].
    Pending,
    /// The latest save failed. The music is unaffected and the daemon keeps
    /// retrying; see [`SessionDurability::error`].
    Failing,
}

/// The daemon session's save state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SessionDurability {
    pub state: SessionSaveState,
    /// The revision of the newest session on disk: every authoring change up
    /// to it survives a daemon restart. `None` before this daemon has saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_revision: Option<RuntimeRevision>,
    /// How long the oldest change not yet on disk has waited, in
    /// milliseconds. Present while `Pending` or `Failing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind_ms: Option<u64>,
    /// Why the latest save failed. Present while `Failing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
