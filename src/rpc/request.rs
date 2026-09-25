//! Client request envelopes and the commands a daemon accepts.

use super::{
    ControlWriteIntent, DescribeModuleQuery, InspectionQuery, ModuleTypeQuery, MutationTicket,
    RpcSubscriptionTopic, RuntimeRevision, SnapshotDelivery, RPC_SCHEMA_VERSION,
};
use crate::{ControlValue, Invention};
use serde::{Deserialize, Serialize};

/// Default for [`RpcCommand::LoadInvention::frozen`]: lockfile validation on.
fn default_frozen() -> bool {
    true
}

/// A client request envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcRequest {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Optional precondition: apply this request only while the daemon is at
    /// exactly this revision (see [`RevisionTracker::check`]).
    ///
    /// Omitting it makes the call **unconditional** — the documented behavior
    /// for every client that predates revisions, and for a caller that
    /// deliberately means "apply regardless". A mismatch is refused with
    /// [`RpcErrorCode::RevisionConflict`] and nothing is mutated.
    ///
    /// [`RevisionTracker::check`]: super::RevisionTracker::check
    /// [`RpcErrorCode::RevisionConflict`]: super::RpcErrorCode::RevisionConflict
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<RuntimeRevision>,
    /// Optional recovery identity for an authoring command, reused unchanged
    /// when the command is retried after a lost response (see
    /// [`MutationLedger`]). A retry carrying it is answered with the
    /// original outcome instead of running again.
    ///
    /// [`MutationLedger`]: super::MutationLedger
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<MutationTicket>,
    #[serde(flatten)]
    pub payload: RpcRequestPayload,
}

impl RpcRequest {
    pub fn new(command: RpcCommand) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            request_id: None,
            expected_revision: None,
            mutation: None,
            payload: RpcRequestPayload::Command(command),
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Requires the daemon to be at `revision` for this request to apply.
    pub fn expecting(mut self, revision: RuntimeRevision) -> Self {
        self.expected_revision = Some(revision);
        self
    }

    /// Attaches the recovery ticket this command keeps across retries.
    pub fn with_mutation(mut self, ticket: MutationTicket) -> Self {
        self.mutation = Some(ticket);
        self
    }
}

/// Top-level client request payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcRequestPayload {
    Command(RpcCommand),
    Subscribe {
        topics: Vec<RpcSubscriptionTopic>,
    },
    GetSnapshot,
    /// Poll the daemon's bounded event log for events newer than `after` (the
    /// `latest_seq` from a previous poll; `None` returns everything still
    /// buffered). A request/response alternative to `Subscribe` for clients
    /// that cannot hold a streaming socket open — notably MCP.
    PollEvents {
        #[serde(default)]
        after: Option<u64>,
    },
    /// Read the latest sampled output levels. High-rate `MeterLevel` events are
    /// broadcast to streaming subscribers only (never the event log), so a
    /// poll-only client — notably MCP — reads current levels here instead of
    /// draining them from the event stream (FUG-239 #5).
    GetMeters,
    /// Connect-time handshake: asks the daemon to report its
    /// [`DaemonIdentity`] so the client can confirm it reached a compatible
    /// daemon before driving it. Answered regardless of the request's
    /// `schema_version` so a client can diagnose a schema gap rather than get
    /// an opaque rejection.
    ///
    /// [`DaemonIdentity`]: super::DaemonIdentity
    Hello,
}

/// A single control write in a [`RpcCommand::SetControls`] batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlWrite {
    pub module_id: String,
    pub key: String,
    pub value: ControlValue,
    /// Whether this write sets the module's new starting state or is a live
    /// gesture. Defaults to [`ControlWriteIntent::Author`] for wire
    /// back-compatibility with clients that omit it.
    #[serde(default)]
    pub intent: ControlWriteIntent,
}

impl ControlWrite {
    /// An authoring write: the value becomes the module's new starting state.
    pub fn new(module_id: impl Into<String>, key: impl Into<String>, value: ControlValue) -> Self {
        Self {
            module_id: module_id.into(),
            key: key.into(),
            value,
            intent: ControlWriteIntent::Author,
        }
    }

    /// A live performance gesture: applied and announced, but not recorded in
    /// the retained document and not an authoring change.
    pub fn performed(
        module_id: impl Into<String>,
        key: impl Into<String>,
        value: ControlValue,
    ) -> Self {
        Self {
            intent: ControlWriteIntent::Perform,
            ..Self::new(module_id, key, value)
        }
    }
}

/// Commands accepted by the runtime daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RpcCommand {
    LoadInvention {
        invention: Box<Invention>,
        /// Base path for resolving the document's relative development and
        /// asset references. `Invention::source_path` does not cross the wire,
        /// so file-loaded documents pass their path here (as `ReloadInvention`
        /// does). `None` for inline documents with no relative references.
        #[serde(default)]
        source_path: Option<String>,
        /// When true (the default), the daemon validates `fugue.lock.json`
        /// integrity before loading and refuses on a mismatch. Defaulted for
        /// wire back-compatibility with clients that omit it.
        #[serde(default = "default_frozen")]
        frozen: bool,
        /// When true, the daemon stops (unloads) the invention once a
        /// one-shot playthrough ends. Defaulted for wire back-compat.
        #[serde(default)]
        stop_on_end: bool,
        /// Module whose `ended` control is authoritative for `stop_on_end`;
        /// `None` watches every module exposing one (any true wins).
        #[serde(default)]
        end_source: Option<String>,
    },
    UnloadInvention,
    SetControl {
        module_id: String,
        key: String,
        value: ControlValue,
        /// Whether this write sets the module's new starting state (recorded
        /// in the retained document, advances the revision) or is a live
        /// performance gesture (applied and announced only). Defaults to
        /// [`ControlWriteIntent::Author`] for wire back-compatibility.
        #[serde(default)]
        intent: ControlWriteIntent,
    },
    /// Apply several control writes in one round trip. The daemon applies them
    /// in order within a single command, so a multi-control conducting gesture
    /// (cells + tempo + dynamics) lands together rather than smeared across N
    /// separate requests. Each write coerces and emits a `ControlChanged` event
    /// exactly as a single [`RpcCommand::SetControl`] would.
    SetControls {
        writes: Vec<ControlWrite>,
    },
    AddModule {
        id: String,
        module_type: String,
        #[serde(default)]
        config: serde_json::Value,
    },
    RemoveModule {
        id: String,
    },
    Connect {
        from: String,
        from_port: String,
        to: String,
        to_port: String,
    },
    Disconnect {
        from: String,
        from_port: String,
        to: String,
        to_port: String,
    },
    SwapModule {
        id: String,
        module_type: String,
        #[serde(default)]
        config: serde_json::Value,
        #[serde(default)]
        preserve_connections: bool,
    },
    /// Reload a full invention document into the running graph as a diff of
    /// runtime mutations, keeping the audio stream alive and preserving the
    /// state of modules the diff does not touch. Falls back to a clean
    /// rebuild when the diff cannot be applied; loads normally when nothing
    /// is running. An invalid document is rejected with playback continuing
    /// on the last good version.
    ReloadInvention {
        invention: Box<Invention>,
        /// Base path for resolving the document's relative development and
        /// asset references. `Invention::source_path` does not cross the
        /// wire, so file-loaded documents pass their path here.
        #[serde(default)]
        source_path: Option<String>,
        /// When true (the default), the daemon validates `fugue.lock.json`
        /// integrity before reloading and refuses on a mismatch.
        #[serde(default = "default_frozen")]
        frozen: bool,
    },
    /// Inspect bounded authored selections with revision and source context.
    InspectInvention {
        query: InspectionQuery,
    },
    /// Retrieve the full retained authored snapshot.
    GetInvention {
        delivery: SnapshotDelivery,
    },
    /// Write the daemon's retained declarative document — the authored
    /// invention updated by runtime mutations — to a file. Lossless:
    /// developments, assets, title/description, and the exposed
    /// inputs/outputs/controls sections are preserved, and control changes
    /// appear in module configs.
    SaveInvention {
        /// Destination file path. Clients should pass an absolute path; a
        /// relative path resolves against the daemon's working directory.
        path: String,
    },
    InstallPackage(PackageInstallRequest),
    ListPackages,
    /// Discover available developments without registering or playing them.
    ListDevelopments {
        query: crate::pkg::content::ContentListQuery,
    },
    /// Inspect an exact reusable development and its authored aliases.
    DescribeDevelopment {
        query: crate::pkg::content::ContentDetailQuery,
    },
    /// Discover complete playable inventions without loading them.
    ListExamples {
        query: crate::pkg::content::ContentListQuery,
    },
    /// Inspect an exact playable invention without starting playback.
    DescribeExample {
        query: crate::pkg::content::ContentDetailQuery,
    },
    /// Resolve an exact daemon-local invention reference and start playback.
    LoadExample {
        query: crate::pkg::content::ContentDetailQuery,
        #[serde(default)]
        stop_on_end: bool,
        #[serde(default)]
        end_source: Option<String>,
    },
    /// Discover registered types; defaults to a terse index.
    DescribeModuleTypes(ModuleTypeQuery),
    /// Inspect type defaults, supplied config, or an existing module instance.
    DescribeModule(DescribeModuleQuery),
    /// Ask the shared daemon to persist its session and shut down cleanly.
    ///
    /// Because a spawned shared daemon outlives the client that started it (so
    /// one client exiting never cuts another's audio), this is how a client
    /// deliberately stops it — there is no owning terminal to Ctrl+C when the
    /// daemon was spawned detached.
    Shutdown,
}

/// Package installation request placeholder for future package discovery work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageInstallRequest {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
