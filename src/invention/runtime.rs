//! Invention runtime for executing modular synthesis inventions.

use crate::modules::{AudioBackend, AudioDriver};
use crate::registry::ModuleRegistry;
use crate::{ControlMeta, Module};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::graph::{GraphCommand, RoutingConnection, SignalGraph, SinkInstance};
use super::handles::InventionHandles;

/// Type alias for module instances stored in the runtime.
pub type ModuleInstance = Arc<Mutex<dyn Module + Send>>;

/// Validates that a module has the specified output port.
pub(crate) fn validate_output_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.lock().unwrap();
    if !m.outputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have output port '{}'. Available: {:?}",
            m.name(),
            port,
            m.outputs()
        )
        .into());
    }
    Ok(())
}

/// Validates that a module has the specified input port.
pub(crate) fn validate_input_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.lock().unwrap();
    if !m.inputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have input port '{}'. Available: {:?}",
            m.name(),
            port,
            m.inputs()
        )
        .into());
    }
    Ok(())
}

/// A prepared invention ready to run.
pub struct InventionRuntime {
    /// All modules in the invention.
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Sink modules that drive processing.
    pub(crate) sinks: IndexMap<String, SinkInstance>,
    /// Signal routing connections.
    pub(crate) routing: Vec<RoutingConnection>,
    /// Module registry for building new modules at runtime.
    pub(crate) registry: ModuleRegistry,
    /// Sample rate for building new modules at runtime.
    pub(crate) sample_rate: u32,
}

impl InventionRuntime {
    /// Starts audio playback using the default AudioDriver.
    pub fn start(self) -> Result<RunningInvention, Box<dyn std::error::Error>> {
        let audio = AudioDriver::new()?;
        self.start_with_backend(audio)
    }

    /// Starts audio playback with a custom audio backend.
    ///
    /// This allows using alternative audio backends such as file writers,
    /// network streamers, or null outputs for testing.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Use a custom audio backend
    /// let backend = MyCustomBackend::new();
    /// let running = runtime.start_with_backend(backend)?;
    /// ```
    pub fn start_with_backend<B: AudioBackend + 'static>(
        self,
        mut backend: B,
    ) -> Result<RunningInvention, Box<dyn std::error::Error>> {
        // Build input map: for each module, collect all connections that feed into it
        let mut input_map: std::collections::HashMap<String, Vec<RoutingConnection>> =
            std::collections::HashMap::new();

        for conn in &self.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_default()
                .push(conn.clone());
        }

        let (command_tx, command_rx) = mpsc::channel();

        let graph = SignalGraph {
            modules: self.modules,
            sinks: self.sinks,
            input_map,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            topo_dirty: true,
        };

        let graph_arc = Arc::new(Mutex::new(graph));

        // Pass a closure that calls process_sample on the graph
        let graph_clone = graph_arc.clone();
        backend.start(Box::new(move || {
            graph_clone.lock().unwrap().process_sample()
        }))?;

        Ok(RunningInvention {
            backend: Box::new(backend),
            graph: graph_arc,
            command_tx,
            registry: self.registry,
            sample_rate: self.sample_rate,
        })
    }
}

/// Error type for graph command operations.
#[derive(Debug)]
pub enum GraphCommandError {
    /// The audio thread has stopped, so commands can no longer be delivered.
    AudioThreadStopped,
    /// The requested module type is not registered.
    UnknownModuleType(String),
    /// The module factory failed to build the module.
    ModuleBuildFailed(String),
    /// The referenced module does not exist in the graph.
    UnknownModule(String),
    /// The referenced port does not exist on the module.
    InvalidPort(String),
    /// A module control operation failed (invalid key, etc.).
    ControlError(String),
}

impl std::fmt::Display for GraphCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphCommandError::AudioThreadStopped => {
                write!(f, "audio thread has stopped; command not delivered")
            }
            GraphCommandError::UnknownModuleType(t) => {
                write!(f, "unknown module type: {}", t)
            }
            GraphCommandError::ModuleBuildFailed(msg) => {
                write!(f, "module build failed: {}", msg)
            }
            GraphCommandError::UnknownModule(id) => {
                write!(f, "unknown module: {}", id)
            }
            GraphCommandError::InvalidPort(msg) => {
                write!(f, "invalid port: {}", msg)
            }
            GraphCommandError::ControlError(msg) => {
                write!(f, "control error: {}", msg)
            }
        }
    }
}

impl std::error::Error for GraphCommandError {}

/// A running invention with audio output.
pub struct RunningInvention {
    backend: Box<dyn AudioBackend>,
    graph: Arc<Mutex<SignalGraph>>,
    command_tx: mpsc::Sender<GraphCommand>,
    registry: ModuleRegistry,
    sample_rate: u32,
}

impl RunningInvention {
    /// Stops audio playback.
    pub fn stop(mut self) {
        self.backend.stop();
    }

    /// Sends a command to the audio thread for graph mutation.
    pub(crate) fn send_command(&self, cmd: GraphCommand) -> Result<(), GraphCommandError> {
        self.command_tx
            .send(cmd)
            .map_err(|_| GraphCommandError::AudioThreadStopped)
    }

    /// Sets a module's input port to a specific value.
    ///
    /// The command is sent to the audio thread and applied at the start of the
    /// next sample. This is fire-and-forget: if the module or port doesn't exist,
    /// the command is silently ignored on the audio thread.
    pub fn set_module_input(
        &self,
        module_id: impl Into<String>,
        port: impl Into<String>,
        value: f32,
    ) -> Result<(), GraphCommandError> {
        self.send_command(GraphCommand::SetModuleInput {
            module_id: module_id.into(),
            port: port.into(),
            value,
        })
    }

    /// Adds a new module to the running graph.
    ///
    /// The module is built on the main thread using the registry, then sent
    /// to the audio thread via the command queue. Handles are returned immediately
    /// (they use `Arc<Mutex<T>>` internally and work regardless of graph state).
    ///
    /// If `module_id` already exists, the old module is replaced (hot-swap).
    pub fn add_module(
        &self,
        module_id: impl Into<String>,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<InventionHandles, GraphCommandError> {
        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }

        let result = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        let module_id = module_id.into();

        // Collect handles with flat keys: "module_id.handle_name"
        let mut handle_map = HashMap::new();
        for (handle_name, handle) in result.handles {
            let key = format!("{}.{}", module_id, handle_name);
            handle_map.insert(key, handle);
        }

        self.send_command(GraphCommand::AddModule {
            module_id,
            module: result.module,
            sink: result.sink,
        })?;

        Ok(InventionHandles::new(handle_map))
    }

    /// Adds a connection between two modules in the running graph.
    ///
    /// Validates that both modules exist and have the specified ports before
    /// sending the command to the audio thread. This gives callers immediate,
    /// actionable errors.
    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        // Lock graph to validate modules and ports
        {
            let graph = self.graph.lock().unwrap();

            // Validate source module exists and has the output port
            let source = graph
                .modules
                .get(from_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(from_module.to_string()))?;
            {
                let m = source.lock().unwrap();
                if !m.outputs().contains(&from_port) {
                    return Err(GraphCommandError::InvalidPort(format!(
                        "module '{}' does not have output port '{}' (available: {:?})",
                        from_module,
                        from_port,
                        m.outputs()
                    )));
                }
            }

            // Validate dest module exists and has the input port
            let dest = graph
                .modules
                .get(to_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(to_module.to_string()))?;
            {
                let m = dest.lock().unwrap();
                if !m.inputs().contains(&to_port) {
                    return Err(GraphCommandError::InvalidPort(format!(
                        "module '{}' does not have input port '{}' (available: {:?})",
                        to_module,
                        to_port,
                        m.inputs()
                    )));
                }
            }
        }

        self.send_command(GraphCommand::AddConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })
    }

    /// Disconnects two modules in the running graph.
    ///
    /// This is fire-and-forget: if the connection doesn't exist, the command is
    /// silently ignored on the audio thread.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        self.send_command(GraphCommand::RemoveConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })
    }

    /// Lists the controls available on a specific module.
    pub fn list_controls(&self, module_id: &str) -> Result<Vec<ControlMeta>, GraphCommandError> {
        let graph = self.graph.lock().unwrap();
        let module = graph
            .modules
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        let m = module.lock().unwrap();
        Ok(m.controls())
    }

    /// Lists controls for all modules in the graph.
    ///
    /// Returns a vec of `(module_id, controls)` pairs, skipping modules with no controls.
    pub fn list_all_controls(&self) -> Vec<(String, Vec<ControlMeta>)> {
        let graph = self.graph.lock().unwrap();
        let mut result = Vec::new();
        for (id, module) in &graph.modules {
            let m = module.lock().unwrap();
            let controls = m.controls();
            if !controls.is_empty() {
                result.push((id.clone(), controls));
            }
        }
        result
    }

    /// Gets the current value of a module control.
    pub fn get_control(&self, module_id: &str, key: &str) -> Result<f32, GraphCommandError> {
        let graph = self.graph.lock().unwrap();
        let module = graph
            .modules
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        let m = module.lock().unwrap();
        m.get_control(key).map_err(GraphCommandError::ControlError)
    }

    /// Sets the value of a module control.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: f32,
    ) -> Result<(), GraphCommandError> {
        let graph = self.graph.lock().unwrap();
        let module = graph
            .modules
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        let mut m = module.lock().unwrap();
        m.set_control(key, value)
            .map_err(GraphCommandError::ControlError)
    }

    /// Removes a module from the running graph.
    ///
    /// This is fire-and-forget: if the module doesn't exist, the command is
    /// silently ignored on the audio thread. All connections referencing the
    /// removed module are cleaned up.
    pub fn remove_module(&self, module_id: impl Into<String>) -> Result<(), GraphCommandError> {
        self.send_command(GraphCommand::RemoveModule {
            module_id: module_id.into(),
        })
    }
}
