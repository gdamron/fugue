use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeModuleInfo {
    pub id: String,
    pub module_type: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConnectionInfo {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub running: bool,
    pub sample_rate: u32,
    pub module_count: usize,
    pub connection_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    pub modules: IndexMap<String, RuntimeModuleInfo>,
    pub connections: Vec<RuntimeConnectionInfo>,
    pub sample_rate: u32,
    pub running: bool,
}

impl RuntimeState {
    pub fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            running: self.running,
            sample_rate: self.sample_rate,
            module_count: self.modules.len(),
            connection_count: self.connections.len(),
        }
    }
}
