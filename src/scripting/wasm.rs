//! wasm-side scripting hooks.
//!
//! `code` modules are executed by the surrounding JS host on wasm builds, so
//! the Rust-side manager is intentionally a no-op placeholder.

use crate::invention::{RuntimeController, RuntimeModuleInfo};

#[derive(Default)]
pub struct ScriptManager;

impl ScriptManager {
    /// wasm hosts run scripts externally, so there is nothing to start here.
    pub fn start_all(&self, _controller: RuntimeController) {}

    /// wasm hosts run scripts externally, so there is nothing to start here.
    pub fn start_module(&self, _controller: RuntimeController, _module: RuntimeModuleInfo) {}

    /// Lifecycle is managed by host JavaScript on wasm builds.
    pub fn reset_module(&self, _module_id: &str) {}

    /// Lifecycle is managed by host JavaScript on wasm builds.
    pub fn stop_module(&self, _module_id: &str) {}

    /// Lifecycle is managed by host JavaScript on wasm builds.
    pub fn stop_all(&self) {}
}
