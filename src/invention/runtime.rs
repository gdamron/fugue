//! Invention runtime for executing modular synthesis inventions.

use crate::agents::AgentManager;
use crate::modules::{AudioBackend, AudioDriver};
use crate::registry::ModuleRegistry;
use crate::scripting::ScriptManager;
use crate::{ControlMeta, ControlSurface, ControlValue, GraphModule};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::graph::{GraphCommand, RoutingConnection, SignalGraph};
use super::handles::InventionHandles;
use super::orchestration::{ModulePorts, OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};

/// Type alias for module instances stored in the runtime.
pub type ModuleInstance = GraphModule;
pub type ControlSurfaceInstance = Arc<dyn ControlSurface + Send + Sync>;

pub(crate) fn module_ports(
    modules: &IndexMap<String, ModuleInstance>,
) -> IndexMap<String, ModulePorts> {
    modules
        .iter()
        .map(|(id, module)| {
            (
                id.clone(),
                ModulePorts {
                    inputs: module
                        .module()
                        .inputs()
                        .iter()
                        .map(|port| (*port).to_string())
                        .collect(),
                    outputs: module
                        .module()
                        .outputs()
                        .iter()
                        .map(|port| (*port).to_string())
                        .collect(),
                },
            )
        })
        .collect()
}

/// Validates that a module has the specified output port.
pub(crate) fn validate_output_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.module();
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
    let m = module.module();
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
    pub(crate) sinks: Vec<String>,
    /// Shared runtime control surfaces keyed by module id.
    pub(crate) control_surfaces: IndexMap<String, ControlSurfaceInstance>,
    /// Signal routing connections.
    pub(crate) routing: Vec<RoutingConnection>,
    /// Module registry for building new modules at runtime.
    pub(crate) registry: ModuleRegistry,
    /// Sample rate for building new modules at runtime.
    pub(crate) sample_rate: u32,
    /// Shared runtime snapshot for orchestration and introspection.
    pub(crate) state: Arc<Mutex<RuntimeState>>,
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
        let (command_tx, command_rx) = mpsc::channel();
        let module_ports = Arc::new(Mutex::new(module_ports(&self.modules)));

        let mut graph = SignalGraph {
            modules: self.modules,
            sinks: self.sinks,
            edges: self.routing,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            compiled_routes: Vec::new(),
            sink_indices: Vec::new(),
            topo_dirty: true,
        };

        let control_surfaces = Arc::new(Mutex::new(self.control_surfaces));

        // The audio callback owns the graph, so processing does not lock it.
        backend.start(Box::new(move || graph.process_sample()))?;

        {
            self.state.lock().unwrap().running = true;
        }

        let running = RunningInvention {
            backend: Box::new(backend),
            control_surfaces,
            command_tx,
            registry: self.registry,
            sample_rate: self.sample_rate,
            state: self.state,
            module_ports,
            scripts: ScriptManager::default(),
            agents: AgentManager::default(),
        };
        running.scripts.start_all(running.controller());
        running.agents.start_all(running.controller());
        Ok(running)
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
    control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    command_tx: mpsc::Sender<GraphCommand>,
    registry: ModuleRegistry,
    sample_rate: u32,
    state: Arc<Mutex<RuntimeState>>,
    module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
    scripts: ScriptManager,
    agents: AgentManager,
}

impl RunningInvention {
    /// Stops audio playback.
    pub fn stop(mut self) {
        self.scripts.stop_all();
        self.agents.stop_all();
        self.state.lock().unwrap().running = false;
        self.backend.stop();
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            state: self.state.clone(),
            control_surfaces: self.control_surfaces.clone(),
        }
    }

    pub fn controller(&self) -> RuntimeController {
        RuntimeController {
            snapshot: self.snapshot(),
            registry: self.registry.clone(),
            sample_rate: self.sample_rate,
            graph: None,
            command_tx: Some(self.command_tx.clone()),
            module_ports: self.module_ports.clone(),
        }
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

        if let Some(control_surface) = result.control_surface {
            self.control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.clone(), control_surface);
        }

        self.module_ports.lock().unwrap().insert(
            module_id.clone(),
            ModulePorts {
                inputs: result
                    .module
                    .module()
                    .inputs()
                    .iter()
                    .map(|port| (*port).to_string())
                    .collect(),
                outputs: result
                    .module
                    .module()
                    .outputs()
                    .iter()
                    .map(|port| (*port).to_string())
                    .collect(),
            },
        );

        self.send_command(GraphCommand::AddModule {
            module_id: module_id.clone(),
            module: result.module,
        })?;

        self.state.lock().unwrap().modules.insert(
            module_id.clone(),
            RuntimeModuleInfo {
                id: module_id.clone(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
        );

        if module_type == "code" {
            self.scripts.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }
        if module_type == "agent" {
            self.agents.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }

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
        let ports = self.module_ports.lock().unwrap();
        let source = ports
            .get(from_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(from_module.to_string()))?;
        if !source.outputs.iter().any(|port| port == from_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have output port '{}' (available: {:?})",
                from_module, from_port, source.outputs
            )));
        }
        let dest = ports
            .get(to_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(to_module.to_string()))?;
        if !dest.inputs.iter().any(|port| port == to_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have input port '{}' (available: {:?})",
                to_module, to_port, dest.inputs
            )));
        }
        drop(ports);

        self.send_command(GraphCommand::AddConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;

        self.state
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
        })?;

        self.state.lock().unwrap().connections.retain(|conn| {
            !(conn.from == from_module
                && conn.from_port == from_port
                && conn.to == to_module
                && conn.to_port == to_port)
        });

        Ok(())
    }

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

    /// Sets the value of a module control.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let control_surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        control_surface
            .set_control(key, value)
            .map_err(GraphCommandError::ControlError)
    }

    /// Removes a module from the running graph.
    ///
    /// This is fire-and-forget: if the module doesn't exist, the command is
    /// silently ignored on the audio thread. All connections referencing the
    /// removed module are cleaned up.
    pub fn remove_module(&self, module_id: impl Into<String>) -> Result<(), GraphCommandError> {
        let module_id = module_id.into();
        self.scripts.stop_module(&module_id);
        self.agents.stop_module(&module_id);
        self.control_surfaces
            .lock()
            .unwrap()
            .shift_remove(&module_id);
        self.module_ports.lock().unwrap().shift_remove(&module_id);
        self.send_command(GraphCommand::RemoveModule {
            module_id: module_id.clone(),
        })?;
        let mut state = self.state.lock().unwrap();
        state.modules.shift_remove(&module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        Ok(())
    }
}

impl OrchestrationRuntime for RunningInvention {
    fn status(&self) -> RuntimeStatus {
        self.snapshot().status()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invention::builder::InventionBuilder;
    use crate::invention::format::Invention;
    use crate::SinkOutput;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    struct TickBackend {
        sample_rate: u32,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl TickBackend {
        fn new(sample_rate: u32) -> Self {
            Self {
                sample_rate,
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            }
        }
    }

    impl AudioBackend for TickBackend {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn start(
            &mut self,
            mut sample_fn: Box<dyn FnMut() -> SinkOutput + Send>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let stop = self.stop.clone();
            self.worker = Some(thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = sample_fn();
                    thread::sleep(Duration::from_millis(2));
                }
            }));
            Ok(())
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[test]
    fn running_invention_tracks_runtime_module_mutations() {
        let invention = Invention::from_json(
            r#"{
                "version": "1.0.0",
                "modules": [
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
        )
        .unwrap();

        let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
        let running = runtime
            .start_with_backend(TickBackend::new(48_000))
            .unwrap();

        assert_eq!(running.list_modules().len(), 1);
        running
            .add_module(
                "code1",
                "code",
                &serde_json::json!({
                    "script": "function init() { graph.addModule('osc_live', 'oscillator', { waveform: 'sine', frequency: 220.0 }) }"
                }),
            )
            .unwrap();

        thread::sleep(Duration::from_millis(50));

        assert!(running
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_live"));

        running.remove_module("osc_live").unwrap();
        assert!(!running
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_live"));

        running.stop();
    }

    #[test]
    fn running_invention_code_tick_updates_controls() {
        let invention = Invention::from_json(
            r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "tick_hz": 20.0,
                            "script": "function tick() { graph.setControl('code1', 'last_error', 'tick-ran') }"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
        )
        .unwrap();

        let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
        let running = runtime
            .start_with_backend(TickBackend::new(48_000))
            .unwrap();

        thread::sleep(Duration::from_millis(120));

        assert_eq!(
            running.get_control("code1", "last_error").unwrap(),
            ControlValue::String("tick-ran".to_string())
        );

        running.stop();
    }

    #[test]
    fn running_invention_supports_returned_lifecycle_object() {
        let invention = Invention::from_json(
            r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "(() => ({ init() { graph.addModule('osc_from_object_live', 'oscillator', { waveform: 'sine', frequency: 330.0 }) } }))()"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
        )
        .unwrap();

        let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
        let running = runtime
            .start_with_backend(TickBackend::new(48_000))
            .unwrap();

        thread::sleep(Duration::from_millis(50));

        assert!(running
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_object_live"));

        running.stop();
    }

    #[test]
    fn running_invention_keeps_legacy_globalthis_hooks_working() {
        let invention = Invention::from_json(
            r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "globalThis.init = function () { graph.addModule('osc_from_legacy_live', 'oscillator', { waveform: 'sine', frequency: 260.0 }) }"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
        )
        .unwrap();

        let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
        let running = runtime
            .start_with_backend(TickBackend::new(48_000))
            .unwrap();

        thread::sleep(Duration::from_millis(50));

        assert!(running
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_legacy_live"));

        running.stop();
    }
}
