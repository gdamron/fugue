use crate::invention::{RuntimeController, RuntimeModuleInfo};

// Braces, not a unit struct, so `AgentManager::default()` reads the same as
// the native manager's constructor at its call sites.
#[derive(Default)]
pub struct AgentManager {}

impl AgentManager {
    pub fn set_stubbed(&self, _stubbed: bool) {}
    pub fn start_all(&self, _controller: RuntimeController) {}
    pub fn start_module(&self, _controller: RuntimeController, _module: RuntimeModuleInfo) {}
    pub fn stop_module(&self, _module_id: &str) {}
    pub fn stop_all(&self) {}
}
