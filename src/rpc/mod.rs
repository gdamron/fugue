//! Typed runtime RPC schema shared by Fugue clients.
//!
//! This module defines the JSON payloads used by future daemon transports. It
//! intentionally contains no socket, WebSocket, or MCP server implementation.

use crate::{
    Connection, ControlMeta, ControlValue, GraphCommandError, Invention, ModuleRegistry,
    RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

mod identity;
pub use identity::{
    verify_daemon_identity, BuildFingerprint, DaemonIdentity, IdentityMismatch,
};

/// Current runtime RPC schema version.
pub const RPC_SCHEMA_VERSION: u32 = 1;

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
    #[serde(flatten)]
    pub payload: RpcRequestPayload,
}

impl RpcRequest {
    pub fn new(command: RpcCommand) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            request_id: None,
            payload: RpcRequestPayload::Command(command),
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

/// Top-level client request payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcRequestPayload {
    Command(RpcCommand),
    Subscribe { topics: Vec<RpcSubscriptionTopic> },
    GetSnapshot,
    /// Connect-time handshake: asks the daemon to report its
    /// [`DaemonIdentity`] so the client can confirm it reached a compatible
    /// daemon before driving it. Answered regardless of the request's
    /// `schema_version` so a client can diagnose a schema gap rather than get
    /// an opaque rejection.
    Hello,
}

/// Commands accepted by the runtime daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RpcCommand {
    LoadInvention {
        invention: Box<Invention>,
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
    DescribeModuleTypes,
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

/// A server response envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcResponse {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub payload: RpcResponsePayload,
}

impl RpcResponse {
    pub fn ok(request_id: Option<String>, payload: RpcResponsePayload) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            request_id,
            payload,
        }
    }

    pub fn error(request_id: Option<String>, error: RpcError) -> Self {
        Self::ok(request_id, RpcResponsePayload::Error(error))
    }
}

/// Top-level server response payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcResponsePayload {
    Ack,
    Snapshot(RuntimeFullSnapshot),
    Packages(PackageList),
    ModuleTypes(ModuleTypeList),
    Reload(ReloadOutcome),
    Saved(SaveReport),
    /// The daemon's identity, in reply to [`RpcRequestPayload::Hello`]. Nested
    /// (not flattened) so `DaemonIdentity::schema_version` does not collide with
    /// the response envelope's own `schema_version`.
    Identity { identity: DaemonIdentity },
    Error(RpcError),
}

/// Response payload for [`RpcCommand::SaveInvention`].
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

/// A server-pushed event envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcEvent {
    pub schema_version: u32,
    #[serde(flatten)]
    pub payload: RpcEventPayload,
}

impl RpcEvent {
    pub fn new(payload: RpcEventPayload) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            payload,
        }
    }
}

/// Event stream topics clients can subscribe to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcSubscriptionTopic {
    ControlChanges,
    MeterLevels,
    AgentActivity,
    SinkStatus,
    Errors,
    Topology,
}

/// Runtime events emitted by daemon transports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RpcEventPayload {
    ControlChanged {
        module_id: String,
        key: String,
        value: ControlValue,
    },
    MeterLevel {
        sink_id: String,
        left_peak: f32,
        right_peak: f32,
    },
    AgentActivity {
        module_id: String,
        activity: String,
    },
    SinkStatus {
        sink_id: String,
        status: SinkStatusState,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Error(RpcError),
    TopologyChanged,
    Snapshot(RuntimeFullSnapshot),
}

/// Minimal sink interface for transports that collect or broadcast RPC events.
pub trait RpcEventSink: Send + Sync {
    fn emit(&self, event: RpcEvent);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SinkStatusState {
    Starting,
    Running,
    Stopped,
    Error,
}

/// Full runtime state view for inspectors, MCP tools, and future canvases.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeFullSnapshot {
    pub status: RuntimeStatus,
    pub modules: Vec<RuntimeModuleSnapshot>,
    pub connections: Vec<RuntimeConnectionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeModuleSnapshot {
    pub info: RuntimeModuleInfo,
    pub ports: RuntimePortInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<RuntimeControlSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimePortInfo {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeControlSnapshot {
    pub meta: ControlMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ControlValue>,
}

/// Built-in and future package inventory response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageList {
    pub packages: Vec<PackageInfo>,
}

impl PackageList {
    /// Returns the package inventory available from the built-in module registry.
    pub fn built_in(registry: &ModuleRegistry) -> Self {
        let mut module_types: Vec<String> = registry.types().map(str::to_string).collect();
        module_types.sort();
        Self {
            packages: vec![PackageInfo {
                name: "builtin".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                source: PackageSource::BuiltIn,
                module_types,
            }],
        }
    }
}

/// Built-in module type discovery response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleTypeList {
    pub module_types: Vec<ModuleTypeInfo>,
}

impl ModuleTypeList {
    /// Returns port and control metadata for module types in the built-in registry.
    pub fn built_in(registry: &ModuleRegistry, sample_rate: u32) -> Self {
        let mut type_names: Vec<&str> = registry.types().collect();
        type_names.sort();

        let module_types = type_names
            .into_iter()
            .map(|type_name| {
                let config = serde_json::Value::Null;
                match registry.build(type_name, sample_rate, &config) {
                    Ok(result) => {
                        let module = result.module.module();
                        ModuleTypeInfo {
                            type_name: type_name.to_string(),
                            inputs: module
                                .inputs()
                                .iter()
                                .map(|port| port.to_string())
                                .collect(),
                            outputs: module
                                .outputs()
                                .iter()
                                .map(|port| port.to_string())
                                .collect(),
                            controls: result
                                .control_surface
                                .as_ref()
                                .map(|surface| surface.controls())
                                .unwrap_or_default(),
                            is_sink: registry.is_sink(type_name),
                        }
                    }
                    Err(_) => ModuleTypeInfo {
                        type_name: type_name.to_string(),
                        inputs: registry
                            .factory_input_ports(type_name)
                            .unwrap_or_default()
                            .iter()
                            .map(|port| port.to_string())
                            .collect(),
                        outputs: registry
                            .factory_output_ports(type_name)
                            .unwrap_or_default()
                            .iter()
                            .map(|port| port.to_string())
                            .collect(),
                        controls: Vec::new(),
                        is_sink: registry.is_sink(type_name),
                    },
                }
            })
            .collect();

        Self { module_types }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleTypeInfo {
    pub type_name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub controls: Vec<ControlMeta>,
    pub is_sink: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub source: PackageSource,
    pub module_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PackageSource {
    BuiltIn,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Unsupported, message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    IncompatibleSchemaVersion,
    AudioThreadStopped,
    UnknownModuleType,
    ModuleBuildFailed,
    UnknownModule,
    InvalidPort,
    ControlError,
    Unsupported,
    Internal,
}

pub fn validate_schema_version(schema_version: u32) -> Result<(), RpcError> {
    if schema_version == RPC_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(RpcError::new(
            RpcErrorCode::IncompatibleSchemaVersion,
            format!(
                "incompatible RPC schema version: client sent {}, server requires {}",
                schema_version, RPC_SCHEMA_VERSION
            ),
        ))
    }
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
        };
        Self::new(code, error.to_string())
    }
}

impl From<Connection> for RuntimeConnectionInfo {
    fn from(connection: Connection) -> Self {
        Self {
            from: connection.from,
            from_port: connection.from_port.unwrap_or_default(),
            to: connection.to,
            to_port: connection.to_port.unwrap_or_default(),
        }
    }
}

#[cfg(feature = "rpc-schema")]
pub mod schema {
    use super::{RpcEvent, RpcRequest, RpcResponse};
    use schemars::{schema_for, Schema};

    pub fn runtime_rpc_schema() -> Schema {
        schema_for!((RpcRequest, RpcResponse, RpcEvent))
    }
}

#[cfg(test)]
mod tests;
