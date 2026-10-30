//! Server response envelopes and their payloads.

use super::{
    ApplyEditsReport, AuthoredSnapshot, DaemonIdentity, EventPage, InspectionPage, MeterReading,
    ModuleDescription, ModuleTypeList, MutationReceipt, NotePage, PackageList, PendingRequests,
    RequestStatus, RpcError, RuntimeFullSnapshot, RuntimeRevision, RPC_SCHEMA_VERSION,
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
    /// A mutation that committed, acknowledged compactly because the request
    /// asked for [`MutationReply::Receipt`]. Takes the place of `Snapshot`.
    ///
    /// [`MutationReply::Receipt`]: super::MutationReply::Receipt
    Receipt(MutationReceipt),
    /// A timed request's status: the reply to a command sent with a
    /// `when`, which queued it, and to `GetRequest`.
    Request(RequestStatus),
    /// The reply to `ListPendingRequests`.
    PendingRequests(PendingRequests),
    Events(EventPage),
    /// The reply to [`RpcRequestPayload::PollNotes`].
    ///
    /// [`RpcRequestPayload::PollNotes`]: super::RpcRequestPayload::PollNotes
    Notes(NotePage),
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
    /// A scope just opened, in reply to `OpenScope`.
    ScopeOpened(super::ScopeInfo),
    /// A polled scope's bins since the last read, in reply to `ReadScope`.
    ScopeFrame(super::ScopeFrame),
    /// The session's focus, in reply to `SetFocus` and `GetFocus`. None when
    /// nothing is selected.
    Focus {
        focus: Option<super::Focus>,
    },
    /// The pinned controls, in reply to `EditPins` and `GetPins`.
    Pins(super::PinList),
    /// The reply to `GetSummary`.
    Summary {
        summary: super::StatusSummary,
    },
    /// A committed [`RpcCommand::InstallPackage`].
    ///
    /// [`RpcCommand::InstallPackage`]: super::RpcCommand::InstallPackage
    PackageInstalled {
        installed: super::PackageInstallReport,
    },
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
    /// A committed [`RpcCommand::ApplyEdits`] batch. The envelope's
    /// `revision` is the revision it committed at.
    ///
    /// [`RpcCommand::ApplyEdits`]: super::RpcCommand::ApplyEdits
    EditsApplied(ApplyEditsReport),
    Saved(SaveReport),
    /// Every change applied before an [`RpcCommand::FlushSession`] is on disk.
    ///
    /// [`RpcCommand::FlushSession`]: super::RpcCommand::FlushSession
    SessionFlushed(super::SessionDurability),
    /// A bounded view of authored state, including its session and revision.
    InventionInspection {
        page: Box<InspectionPage>,
    },
    /// A revision-stamped authored snapshot, never a flattened runtime graph.
    AuthoredSnapshot {
        snapshot: Box<AuthoredSnapshot>,
    },
    /// The running session's request log, in reply to
    /// [`RpcCommand::ExportRequestLog`].
    ///
    /// [`RpcCommand::ExportRequestLog`]: super::RpcCommand::ExportRequestLog
    RequestLog {
        log: crate::RequestLog,
    },
    /// The daemon's identity, in reply to [`RpcRequestPayload::Hello`]. Nested
    /// (not flattened) so `DaemonIdentity::schema_version` does not collide with
    /// the response envelope's own `schema_version`.
    ///
    /// [`RpcRequestPayload::Hello`]: super::RpcRequestPayload::Hello
    Identity {
        identity: DaemonIdentity,
    },
    /// The answer to a retried ticketed command whose original already
    /// committed: nothing ran again. `committed_at` is the revision the
    /// original produced; re-read state for anything the original reply
    /// carried. Named apart from the envelope's own `revision`, which this
    /// payload is flattened beside.
    MutationCommitted {
        mutation_id: String,
        committed_at: RuntimeRevision,
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
    /// The runtime after the reload. Omitted when the request asked for
    /// [`MutationReply::Receipt`]; the envelope's revision still says where the
    /// reload landed.
    ///
    /// [`MutationReply::Receipt`]: super::MutationReply::Receipt
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<RuntimeFullSnapshot>,
}
