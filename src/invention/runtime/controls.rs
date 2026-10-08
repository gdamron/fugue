//! Control reads and writes on a running invention.

use super::{GraphCommandError, RunningInvention};
use crate::{ControlMeta, ControlValue};

impl RunningInvention {
    /// Lists the controls available on a specific module.
    pub fn list_controls(&self, module_id: &str) -> Result<Vec<ControlMeta>, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let control_surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        Ok(control_surface.controls())
    }

    /// Lists controls for all modules in the graph.
    ///
    /// Returns a vec of `(module_id, controls)` pairs, skipping modules with no controls.
    pub fn list_all_controls(&self) -> Vec<(String, Vec<ControlMeta>)> {
        let controls = self.control_surfaces.lock().unwrap();
        let mut result = Vec::new();
        for (id, control_surface) in controls.iter() {
            let metadata = control_surface.controls();
            if !metadata.is_empty() {
                result.push((id.clone(), metadata));
            }
        }
        result
    }

    /// The config kind a reload reads `key` of `module_id` as, when the
    /// running module declares it as a control.
    pub(crate) fn control_kind(
        &self,
        module_id: &str,
        key: &str,
    ) -> Option<crate::module_config::ConfigKind> {
        let surfaces = self.control_surfaces.lock().unwrap();
        let declaration = surfaces.get(module_id)?.declaration(key)?;
        Some(declaration.decl.kind.config_kind())
    }

    /// Gets the current value of a module control.
    pub fn get_control(
        &self,
        module_id: &str,
        key: &str,
    ) -> Result<ControlValue, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let control_surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        control_surface
            .get_control(key)
            .map_err(GraphCommandError::ControlError)
    }

    /// Sets the value of a module control, recording it and announcing it as a
    /// `ControlChanged` event.
    ///
    /// Delegates to [`RuntimeSnapshot::set_control`] so the daemon's single-write
    /// RPC path shares the one choke point (coercion, document recording, and
    /// event emission) with batch writes, scripts, and agents.
    ///
    /// [`RuntimeSnapshot::set_control`]: crate::invention::orchestration::RuntimeSnapshot::set_control
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        self.snapshot().set_control(module_id, key, value)
    }
}
