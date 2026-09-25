//! Full runtime state views served to inspectors and clients.

use crate::{
    Connection, ControlMeta, ControlValue, RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeStatus,
};
use serde::{Deserialize, Serialize};

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
