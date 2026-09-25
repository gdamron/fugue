//! Server response envelopes and their payloads.

use super::{
    AuthoredSnapshot, DaemonIdentity, EventPage, InspectionPage, MeterReading, ModuleDescription,
    ModuleTypeList, PackageList, RpcError, RuntimeFullSnapshot, RuntimeRevision,
    RPC_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

/// A server response envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcResponse {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The daemon's revision as of this reply, stamped on **every** response —
    /// reads, mutation acknowledgements, and errors alike — so a client always
    /// holds a fresh token to precondition its next edit on, and a rejected
    /// client learns where the daemon stands without a second round trip.
    ///
    /// `None` only from a peer that does not track revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RuntimeRevision>,
    #[serde(flatten)]
    pub payload: RpcResponsePayload,
}

impl RpcResponse {
    pub fn ok(request_id: Option<String>, payload: RpcResponsePayload) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            request_id,
            revision: None,
            payload,
        }
    }

    pub fn error(request_id: Option<String>, error: RpcError) -> Self {
        Self::ok(request_id, RpcResponsePayload::Error(error))
    }

    /// Stamps the daemon's current revision onto an outgoing response.
    pub fn with_revision(mut self, revision: RuntimeRevision) -> Self {
        self.revision = Some(revision);
        self
    }
}

/// Top-level server response payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcResponsePayload {
    Ack,
    Snapshot(RuntimeFullSnapshot),
    Events(EventPage),
    /// The latest sampled output levels, in reply to
    /// [`RpcRequestPayload::GetMeters`]. Empty when nothing is playing. A struct
    /// variant (not a newtype over `Vec`) because this enum is internally
    /// tagged, which cannot flatten a sequence.
    ///
    /// [`RpcRequestPayload::GetMeters`]: super::RpcRequestPayload::GetMeters
    Meters {
        meters: Vec<MeterReading>,
    },
    Packages(PackageList),
    /// Available development page, independent of the current registry.
    Developments {
        catalog: crate::pkg::content::ContentPage,
    },
    /// Exact development detail.
    DevelopmentDetail {
        detail: crate::pkg::content::ContentDetail,
    },
    /// Available playable invention page, independent of the running graph.
    Examples {
        catalog: crate::pkg::content::ContentPage,
    },
    /// Exact playable invention detail.
    ExampleDetail {
        detail: crate::pkg::content::ContentDetail,
    },
    /// Versioned content error envelope used by musical-content consumers.
    ContentError {
        error: crate::pkg::content::ContentError,
    },
    ModuleTypes {
        discovery: ModuleTypeList,
    },
    ModuleDescription {
        description: ModuleDescription,
    },
    Reload(ReloadOutcome),
    Saved(SaveReport),
    /// A bounded view of authored state, including its session and revision.
    InventionInspection {
        page: Box<InspectionPage>,
    },
    /// A revision-stamped authored snapshot, never a flattened runtime graph.
    AuthoredSnapshot {
        snapshot: Box<AuthoredSnapshot>,
    },
    /// The daemon's identity, in reply to [`RpcRequestPayload::Hello`]. Nested
    /// (not flattened) so `DaemonIdentity::schema_version` does not collide with
    /// the response envelope's own `schema_version`.
    ///
    /// [`RpcRequestPayload::Hello`]: super::RpcRequestPayload::Hello
    Identity {
        identity: DaemonIdentity,
    },
    Error(RpcError),
}

/// Response payload for [`RpcCommand::SaveInvention`].
///
/// [`RpcCommand::SaveInvention`]: super::RpcCommand::SaveInvention
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SaveReport {
    /// The path the document was written to.
    pub path: String,
    pub modules: usize,
    pub connections: usize,
    pub developments: usize,
}

/// How a [`RpcCommand::ReloadInvention`] landed.
///
/// [`RpcCommand::ReloadInvention`]: super::RpcCommand::ReloadInvention
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReloadMode {
    /// The document was applied as runtime mutations; the audio stream never
    /// stopped and unchanged modules kept their state.
    Diff,
    /// The daemon rebuilt the graph from scratch (nothing was running, or
    /// the diff could not be applied); module state restarted.
    Rebuild,
}

/// Response payload for [`RpcCommand::ReloadInvention`].
///
/// [`RpcCommand::ReloadInvention`]: super::RpcCommand::ReloadInvention
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ReloadOutcome {
    pub mode: ReloadMode,
    /// Why the daemon fell back to a rebuild, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the diff changed; present only for [`ReloadMode::Diff`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::ReloadReport>,
    pub snapshot: RuntimeFullSnapshot,
}
