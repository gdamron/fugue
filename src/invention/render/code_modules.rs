//! Inspection and status reporting for `code` modules in an offline render.

use super::RenderEngine;
use crate::invention::runtime::GraphCommandError;
use crate::ControlValue;

/// Serializable config/state view for a `code` module.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeModuleRuntimeInfo {
    pub id: String,
    pub script: String,
    pub entrypoint: String,
    pub enabled: bool,
    pub tick_hz: f32,
}

impl RenderEngine {
    /// Returns the current config/state for all `code` modules in the graph.
    pub fn list_code_modules(&self) -> Result<Vec<CodeModuleRuntimeInfo>, GraphCommandError> {
        let modules = self.list_modules();
        let mut code_modules = Vec::new();
        for module in modules {
            if module.module_type != "code" {
                continue;
            }

            let script = module
                .config
                .get("script")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let entrypoint = module
                .config
                .get("entrypoint")
                .and_then(|value| value.as_str())
                .unwrap_or("init")
                .to_string();
            let enabled = match self.snapshot().get_control(&module.id, "enabled")? {
                ControlValue::Bool(value) => value,
                _ => true,
            };
            let tick_hz = match self.snapshot().get_control(&module.id, "tick_hz")? {
                ControlValue::Number(value) => value,
                _ => 0.0,
            };

            code_modules.push(CodeModuleRuntimeInfo {
                id: module.id,
                script,
                entrypoint,
                enabled,
                tick_hz,
            });
        }

        Ok(code_modules)
    }

    /// Returns the current config/state for a single `code` module.
    pub fn get_code_module(
        &self,
        module_id: &str,
    ) -> Result<CodeModuleRuntimeInfo, GraphCommandError> {
        self.list_code_modules()?
            .into_iter()
            .find(|module| module.id == module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))
    }

    /// Updates the runtime status string for a `code` module.
    pub fn set_code_module_status(
        &self,
        module_id: &str,
        status: impl Into<String>,
    ) -> Result<(), GraphCommandError> {
        self.snapshot().set_control_transient(
            module_id,
            "status",
            ControlValue::String(status.into()),
        )
    }

    /// Updates the last-error string for a `code` module.
    pub fn set_code_module_error(
        &self,
        module_id: &str,
        error: impl Into<String>,
    ) -> Result<(), GraphCommandError> {
        self.snapshot().set_control_transient(
            module_id,
            "last_error",
            ControlValue::String(error.into()),
        )
    }
}
