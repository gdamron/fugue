use crate::invention::{RuntimeController, RuntimeModuleInfo};

#[derive(Default)]
pub struct AgentManager;

impl AgentManager {
    pub fn start_all(&self, _controller: RuntimeController) {}
    pub fn start_module(&self, _controller: RuntimeController, _module: RuntimeModuleInfo) {}
    pub fn stop_module(&self, _module_id: &str) {}
    pub fn stop_all(&self) {}
}
