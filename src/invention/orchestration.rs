use crate::factory::ModuleBuildResult;
use crate::invention::graph::{GraphCommand, SignalGraph};
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, GraphCommandError,
};
use crate::registry::ModuleRegistry;
use crate::{ControlMeta, ControlValue};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};

/// Read/write orchestration surface shared by live and render runtimes.
pub trait OrchestrationRuntime {
    /// Returns the current runtime status.
    fn status(&self) -> RuntimeStatus;
    /// Returns the current module snapshot.
    fn list_modules(&self) -> Vec<RuntimeModuleInfo>;
    /// Returns the current connection snapshot.
    fn list_connections(&self) -> Vec<RuntimeConnectionInfo>;
    /// Returns control metadata for one module or for all modules with controls.
    fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<ControlMeta>)>, GraphCommandError>;
    /// Reads a control value from a specific module.
    fn get_control(&self, module_id: &str, key: &str) -> Result<ControlValue, GraphCommandError>;
    /// Updates a control value on a specific module.
    fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError>;
}

/// Cloneable read-oriented view over runtime state and control surfaces.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub state: Arc<Mutex<RuntimeState>>,
    pub control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
}

/// Cloneable mutation handle used by orchestration hosts and external APIs.
///
/// Live runtimes route mutations through the audio-thread command queue, while
/// render runtimes apply the same commands directly to the in-memory graph.
#[derive(Clone)]
pub struct RuntimeController {
    pub(crate) snapshot: RuntimeSnapshot,
    pub(crate) registry: ModuleRegistry,
    pub(crate) sample_rate: u32,
    pub(crate) graph: Arc<Mutex<SignalGraph>>,
    pub(crate) command_tx: Option<mpsc::Sender<GraphCommand>>,
}

impl RuntimeSnapshot {
    /// Returns aggregate status for the current invention.
    pub fn status(&self) -> RuntimeStatus {
        self.state.lock().unwrap().status()
    }

    /// Returns a copy of the current module snapshot.
    pub fn list_modules(&self) -> Vec<RuntimeModuleInfo> {
        self.state
            .lock()
            .unwrap()
            .modules
            .values()
            .cloned()
            .collect()
    }

    /// Returns a copy of the current connection snapshot.
    pub fn list_connections(&self) -> Vec<RuntimeConnectionInfo> {
        self.state.lock().unwrap().connections.clone()
    }

    /// Lists controls for a single module or all modules with control surfaces.
    pub fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<ControlMeta>)>, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        if let Some(module_id) = module_id {
            let surface = controls
                .get(module_id)
                .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
            return Ok(vec![(module_id.to_string(), surface.controls())]);
        }

        let mut result = Vec::new();
        for (id, surface) in controls.iter() {
            let metadata = surface.controls();
            if !metadata.is_empty() {
                result.push((id.clone(), metadata));
            }
        }
        Ok(result)
    }

    /// Reads the current value of a module control.
    pub fn get_control(
        &self,
        module_id: &str,
        key: &str,
    ) -> Result<ControlValue, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        surface
            .get_control(key)
            .map_err(GraphCommandError::ControlError)
    }

    /// Sets the current value of a module control.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        surface
            .set_control(key, value)
            .map_err(GraphCommandError::ControlError)
    }
}

impl RuntimeController {
    fn send_or_apply(&self, cmd: GraphCommand) -> Result<(), GraphCommandError> {
        if let Some(command_tx) = &self.command_tx {
            command_tx
                .send(cmd)
                .map_err(|_| GraphCommandError::AudioThreadStopped)
        } else {
            self.graph.lock().unwrap().apply_command(cmd);
            Ok(())
        }
    }

    /// Builds and inserts a module into the current graph.
    ///
    /// Returned handles are flattened as `<module_id>.<handle_name>` to match
    /// the runtime's existing handle naming scheme.
    pub fn add_module(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<HashMap<String, Arc<dyn Any + Send + Sync>>, GraphCommandError> {
        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }

        let ModuleBuildResult {
            module,
            handles,
            control_surface,
            sink,
        } = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        if let Some(control_surface) = control_surface {
            self.snapshot
                .control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.to_string(), control_surface);
        }

        self.send_or_apply(GraphCommand::AddModule {
            module_id: module_id.to_string(),
            module,
            sink,
        })?;

        self.snapshot.state.lock().unwrap().modules.insert(
            module_id.to_string(),
            RuntimeModuleInfo {
                id: module_id.to_string(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
        );

        Ok(handles
            .into_iter()
            .map(|(name, handle)| (format!("{}.{}", module_id, name), handle))
            .collect())
    }

    /// Removes a module and any connections that reference it.
    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        self.snapshot
            .control_surfaces
            .lock()
            .unwrap()
            .shift_remove(module_id);
        self.send_or_apply(GraphCommand::RemoveModule {
            module_id: module_id.to_string(),
        })?;
        let mut state = self.snapshot.state.lock().unwrap();
        state.modules.shift_remove(module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        Ok(())
    }

    /// Connects an output port to an input port after validating both ends.
    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        {
            let graph = self.graph.lock().unwrap();
            let source = graph
                .modules
                .get(from_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(from_module.to_string()))?;
            validate_output_port(source, from_port)
                .map_err(|e| GraphCommandError::InvalidPort(e.to_string()))?;
            let dest = graph
                .modules
                .get(to_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(to_module.to_string()))?;
            validate_input_port(dest, to_port)
                .map_err(|e| GraphCommandError::InvalidPort(e.to_string()))?;
        }

        self.send_or_apply(GraphCommand::AddConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;

        self.snapshot
            .state
            .lock()
            .unwrap()
            .connections
            .push(RuntimeConnectionInfo {
                from: from_module.to_string(),
                from_port: from_port.to_string(),
                to: to_module.to_string(),
                to_port: to_port.to_string(),
            });
        Ok(())
    }

    /// Removes a connection between two ports if present.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        self.send_or_apply(GraphCommand::RemoveConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;
        self.snapshot
            .state
            .lock()
            .unwrap()
            .connections
            .retain(|conn| {
                !(conn.from == from_module
                    && conn.from_port == from_port
                    && conn.to == to_module
                    && conn.to_port == to_port)
            });
        Ok(())
    }
}
