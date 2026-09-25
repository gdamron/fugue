//! [`OrchestrationRuntime`] for a running invention.

use super::{GraphCommandError, RunningInvention};
use crate::invention::orchestration::OrchestrationRuntime;
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeStatus};
use crate::{ControlMeta, ControlValue};

impl OrchestrationRuntime for RunningInvention {
    fn status(&self) -> RuntimeStatus {
        self.with_audio_diagnostics(self.snapshot().status())
    }

    fn list_modules(&self) -> Vec<RuntimeModuleInfo> {
        self.snapshot().list_modules()
    }

    fn list_connections(&self) -> Vec<RuntimeConnectionInfo> {
        self.snapshot().list_connections()
    }

    fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<ControlMeta>)>, GraphCommandError> {
        self.snapshot().list_controls(module_id)
    }

    fn get_control(&self, module_id: &str, key: &str) -> Result<ControlValue, GraphCommandError> {
        self.snapshot().get_control(module_id, key)
    }

    fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        self.snapshot().set_control(module_id, key, value)
    }

    fn set_control_with_intent(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        intent: crate::ControlWriteIntent,
    ) -> Result<(), GraphCommandError> {
        self.snapshot()
            .set_control_with_intent(module_id, key, value, intent)
    }
}
