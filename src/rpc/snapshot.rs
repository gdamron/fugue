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
    /// The clock a request timed in beats on this module counts on when it
    /// names none. Absent when the invention has no clock, and in offline
    /// render snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<TimelineContext>,
}

/// A module's timeline context: the clock that drives it through its
/// connections, which a request timed in beats on it counts on when it
/// names no clock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TimelineContext {
    /// The nearest clock upstream, `distance` connections away (0 for a
    /// clock itself). Ties go to a clock a request can name over one inside
    /// a development, then to the one first in module order.
    Connected { clock: String, distance: u32 },
    /// No clock is connected to the module: the first clock a request can
    /// name, in module order.
    Fallback { clock: String },
    /// The module counts on a clock inside this development, which a
    /// request cannot name yet: the nearest clock driving it, or, with no
    /// clock a request can name anywhere, the first development holding one.
    /// A request timed in beats on it must name a clock.
    Enclosed { development: String },
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
