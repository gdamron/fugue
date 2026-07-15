use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::invention::format::{Connection, Invention, ModuleSpec};
use crate::modules::AudioDiagnosticsSnapshot;
use crate::ControlValue;

/// Serializable description of a module in a running invention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeModuleInfo {
    /// Stable instance id inside the graph.
    pub id: String,
    /// Registered module type used to build this instance.
    pub module_type: String,
    /// Original config payload used to construct the module.
    pub config: serde_json::Value,
}

/// Serializable description of a routed connection in a running invention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeConnectionInfo {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

/// Lightweight runtime status used by orchestration and external APIs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeStatus {
    pub running: bool,
    pub sample_rate: u32,
    pub module_count: usize,
    pub connection_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<AudioDiagnosticsSnapshot>,
}

/// Authoritative runtime-owned snapshot of modules, connections, and status.
#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    pub modules: IndexMap<String, RuntimeModuleInfo>,
    pub connections: Vec<RuntimeConnectionInfo>,
    pub sample_rate: u32,
    pub running: bool,
    /// The authored declarative document this graph was built from, kept in
    /// sync with runtime mutations so the live graph can be written back to
    /// disk without losing developments, assets, title, or the
    /// inputs/outputs/controls sections. `None` for graphs assembled without
    /// a document. Module configs stay as authored (`$asset` references
    /// unresolved); connections are mirrored from `connections` on assembly.
    pub document: Option<Invention>,
}

impl RuntimeState {
    /// Builds a summary view suitable for tooling and scripting APIs.
    pub fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            running: self.running,
            sample_rate: self.sample_rate,
            module_count: self.modules.len(),
            connection_count: self.connections.len(),
            diagnostics: None,
        }
    }

    /// Records a module added, replaced, or swapped at runtime in the
    /// retained document.
    pub(crate) fn document_upsert_module(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match document.modules.iter_mut().find(|spec| spec.id == id) {
            Some(spec) => {
                spec.module_type = module_type.to_string();
                spec.config = config.clone();
            }
            None => document.modules.push(ModuleSpec {
                id: id.to_string(),
                module_type: module_type.to_string(),
                config: config.clone(),
            }),
        }
    }

    /// Removes a module from the retained document.
    pub(crate) fn document_remove_module(&mut self, id: &str) {
        if let Some(document) = self.document.as_mut() {
            document.modules.retain(|spec| spec.id != id);
        }
    }

    /// Writes a control change into the retained document module's config so
    /// a saved document reproduces the value on a cold rebuild.
    pub(crate) fn document_write_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let Some(spec) = document.modules.iter_mut().find(|spec| spec.id == id) else {
            return;
        };
        let value = match value {
            // Widen through the shortest decimal form of the f32 (its
            // Display output) so 0.7f32 lands in the document as 0.7, not
            // 0.699999988079071.
            ControlValue::Number(number) => number
                .to_string()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            ControlValue::Bool(flag) => serde_json::json!(flag),
            ControlValue::String(text) => serde_json::json!(text),
        };
        if !spec.config.is_object() {
            spec.config = serde_json::Value::Object(serde_json::Map::new());
        }
        spec.config
            .as_object_mut()
            .expect("config was just made an object")
            .insert(key.to_string(), value);
    }

    /// Assembles the retained declarative document, mirroring the live
    /// graph's connections. Returns `None` when no document was retained.
    pub fn document(&self) -> Option<Invention> {
        let mut document = self.document.clone()?;
        document.connections = self
            .connections
            .iter()
            .map(|conn| Connection {
                from: conn.from.clone(),
                to: conn.to.clone(),
                from_port: (!conn.from_port.is_empty()).then(|| conn.from_port.clone()),
                to_port: (!conn.to_port.is_empty()).then(|| conn.to_port.clone()),
            })
            .collect();
        Some(document)
    }
}
