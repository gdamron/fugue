//! Typed runtime RPC schema shared by Fugue clients.
//!
//! This module defines the JSON payloads used by future daemon transports. It
//! intentionally contains no socket, WebSocket, or MCP server implementation.

use crate::{
    Connection, ControlMeta, ControlValue, GraphCommandError, Invention, ModuleRegistry,
    RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

/// Current runtime RPC schema version.
pub const RPC_SCHEMA_VERSION: u32 = 1;

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
}

/// Commands accepted by the runtime daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RpcCommand {
    LoadInvention {
        invention: Box<Invention>,
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
    InstallPackage(PackageInstallRequest),
    ListPackages,
    DescribeModuleTypes,
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
    Error(RpcError),
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
mod tests {
    use super::*;
    use crate::{ControlValue, ModuleSpec};

    fn test_invention() -> Invention {
        Invention {
            version: "1.0.0".to_string(),
            title: Some("rpc-test".to_string()),
            description: None,
            developments: Vec::new(),
            assets: std::collections::BTreeMap::new(),
            modules: vec![ModuleSpec {
                id: "dac".to_string(),
                module_type: "dac".to_string(),
                config: serde_json::Value::Null,
            }],
            connections: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            controls: Vec::new(),
            source_path: None,
        }
    }

    #[test]
    fn rpc_commands_round_trip_json() {
        let commands = vec![
            RpcCommand::LoadInvention {
                invention: Box::new(test_invention()),
            },
            RpcCommand::UnloadInvention,
            RpcCommand::SetControl {
                module_id: "osc".to_string(),
                key: "frequency".to_string(),
                value: ControlValue::Number(440.0),
            },
            RpcCommand::AddModule {
                id: "osc".to_string(),
                module_type: "oscillator".to_string(),
                config: serde_json::json!({ "frequency": 440.0 }),
            },
            RpcCommand::RemoveModule {
                id: "osc".to_string(),
            },
            RpcCommand::Connect {
                from: "osc".to_string(),
                from_port: "audio".to_string(),
                to: "dac".to_string(),
                to_port: "audio".to_string(),
            },
            RpcCommand::Disconnect {
                from: "osc".to_string(),
                from_port: "audio".to_string(),
                to: "dac".to_string(),
                to_port: "audio".to_string(),
            },
            RpcCommand::SwapModule {
                id: "osc".to_string(),
                module_type: "lfo".to_string(),
                config: serde_json::json!({ "frequency": 2.0 }),
                preserve_connections: true,
            },
            RpcCommand::InstallPackage(PackageInstallRequest {
                package: "demo".to_string(),
                version: Some("1.2.3".to_string()),
            }),
            RpcCommand::ListPackages,
            RpcCommand::DescribeModuleTypes,
        ];

        for command in commands {
            let request = RpcRequest::new(command.clone()).with_request_id("req-1");
            let json = serde_json::to_string(&request).unwrap();
            let decoded: RpcRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.schema_version, RPC_SCHEMA_VERSION);
            assert_eq!(decoded.payload, RpcRequestPayload::Command(command));
        }
    }

    #[test]
    fn schema_version_rejects_incompatible_clients() {
        let error = validate_schema_version(RPC_SCHEMA_VERSION + 1).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::IncompatibleSchemaVersion);
        assert!(error.message.contains("incompatible RPC schema version"));
    }

    #[test]
    fn graph_errors_map_to_rpc_errors() {
        let error = RpcError::from(GraphCommandError::UnknownModule("osc".to_string()));
        assert_eq!(error.code, RpcErrorCode::UnknownModule);
        assert!(error.message.contains("osc"));
    }

    #[test]
    fn built_in_packages_list_registry_types() {
        let registry = ModuleRegistry::default();
        let packages = PackageList::built_in(&registry);
        assert_eq!(packages.packages.len(), 1);
        assert_eq!(packages.packages[0].source, PackageSource::BuiltIn);
        assert!(packages.packages[0]
            .module_types
            .contains(&"oscillator".to_string()));
    }

    #[test]
    fn built_in_module_types_include_ports_and_controls() {
        let registry = ModuleRegistry::default();
        let module_types = ModuleTypeList::built_in(&registry, 44_100);
        let oscillator = module_types
            .module_types
            .iter()
            .find(|module_type| module_type.type_name == "oscillator")
            .expect("oscillator module type is listed");
        assert!(oscillator.outputs.contains(&"audio".to_string()));
        assert!(oscillator
            .controls
            .iter()
            .any(|control| control.key == "frequency"));

        #[cfg(not(target_arch = "wasm32"))]
        {
            let audio_file_sink = module_types
                .module_types
                .iter()
                .find(|module_type| module_type.type_name == "audio_file_sink")
                .expect("audio_file_sink module type is listed");
            assert!(audio_file_sink.is_sink);
            assert!(audio_file_sink.inputs.contains(&"audio".to_string()));
            assert!(audio_file_sink.outputs.contains(&"audio_left".to_string()));
        }
    }

    #[test]
    fn package_install_placeholder_is_structured_unsupported_error() {
        let error = RpcError::unsupported("package installation is not implemented yet");
        assert_eq!(error.code, RpcErrorCode::Unsupported);
        assert!(error.message.contains("not implemented"));
    }

    #[test]
    fn render_engine_full_snapshot_includes_ports_and_control_values() {
        let json = r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#;
        let mut engine = crate::RenderEngine::new(44_100);
        engine.load_json(json).unwrap();
        engine
            .set_control("osc", "frequency", ControlValue::Number(880.0))
            .unwrap();

        let snapshot = engine.full_snapshot();
        assert_eq!(snapshot.status.module_count, 2);
        assert_eq!(snapshot.connections.len(), 1);

        let osc = snapshot
            .modules
            .iter()
            .find(|module| module.info.id == "osc")
            .expect("oscillator module is present");
        assert!(osc.ports.outputs.contains(&"audio".to_string()));
        assert!(osc.ports.inputs.contains(&"frequency".to_string()));
        let frequency = osc
            .controls
            .iter()
            .find(|control| control.meta.key == "frequency")
            .expect("frequency control is present");
        assert_eq!(frequency.value, Some(ControlValue::Number(880.0)));
    }

    #[cfg(feature = "rpc-schema")]
    #[test]
    fn runtime_rpc_schema_generates() {
        let schema = schema::runtime_rpc_schema();
        let json = serde_json::to_value(schema).unwrap();
        assert!(json.is_object());
    }
}
