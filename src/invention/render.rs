//! Offline invention renderer for host-driven playback.

use indexmap::IndexMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::agents::AgentManager;
use crate::scripting::ScriptManager;
use crate::{ControlValue, Invention, InventionBuilder, ModuleRegistry};

use super::graph::{GraphCommand, RoutingConnection, SignalGraph};
use super::orchestration::{ModulePorts, OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
use super::runtime::{
    module_ports, validate_input_port, validate_output_port, ControlSurfaceInstance,
    GraphCommandError,
};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};

/// Offline renderer for inventions.
///
/// Unlike [`super::runtime::RunningInvention`], this type does not own an audio
/// device. Hosts drive rendering explicitly by providing their own output
/// buffers, which makes the engine suitable for FFI and wasm consumers.
pub struct RenderEngine {
    sample_rate: u32,
    graph: Option<Arc<Mutex<SignalGraph>>>,
    registry: ModuleRegistry,
    state: Arc<Mutex<RuntimeState>>,
    control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
    source_json: Option<String>,
    scripts: ScriptManager,
    agents: AgentManager,
}

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
    /// Creates a new renderer with the provided sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            graph: None,
            registry: ModuleRegistry::default(),
            state: Arc::new(Mutex::new(RuntimeState {
                sample_rate,
                ..RuntimeState::default()
            })),
            control_surfaces: Arc::new(Mutex::new(IndexMap::new())),
            module_ports: Arc::new(Mutex::new(IndexMap::new())),
            source_json: None,
            scripts: ScriptManager::default(),
            agents: AgentManager::default(),
        }
    }

    /// Returns the configured sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            state: self.state.clone(),
            control_surfaces: self.control_surfaces.clone(),
        }
    }

    pub fn controller(&self) -> Option<RuntimeController> {
        Some(RuntimeController {
            snapshot: self.snapshot(),
            registry: self.registry.clone(),
            sample_rate: self.sample_rate,
            graph: Some(self.graph.as_ref()?.clone()),
            command_tx: None,
            module_ports: self.module_ports.clone(),
        })
    }

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
        self.snapshot()
            .set_control(module_id, "status", ControlValue::String(status.into()))
    }

    /// Updates the last-error string for a `code` module.
    pub fn set_code_module_error(
        &self,
        module_id: &str,
        error: impl Into<String>,
    ) -> Result<(), GraphCommandError> {
        self.snapshot()
            .set_control(module_id, "last_error", ControlValue::String(error.into()))
    }

    /// Loads an invention from a parsed value.
    pub fn load_invention(
        &mut self,
        invention: Invention,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let builder = InventionBuilder::new(self.sample_rate);
        let (runtime, _) = builder.build(invention)?;
        self.install_runtime(runtime);
        Ok(())
    }

    /// Loads an invention from JSON text.
    pub fn load_json(&mut self, json: &str) -> Result<(), Box<dyn std::error::Error>> {
        let invention = serde_json::from_str::<Invention>(json)?;
        self.load_invention(invention)?;
        self.source_json = Some(json.to_string());
        Ok(())
    }

    /// Reloads the most recently loaded invention and clears runtime state.
    pub fn reset(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(json) = self.source_json.clone() else {
            return Err("no invention loaded".into());
        };
        self.load_json(&json)
    }

    /// Renders interleaved stereo frames into a caller-provided buffer.
    ///
    /// The buffer length must be even because output is written as
    /// `[left0, right0, left1, right1, ...]`.
    pub fn render_interleaved(
        &mut self,
        output: &mut [f32],
    ) -> Result<usize, Box<dyn std::error::Error>> {
        if output.len() % 2 != 0 {
            return Err("output buffer length must be even".into());
        }

        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| "no invention loaded".to_string())?;
        let mut graph = graph.lock().unwrap();

        for frame in output.chunks_exact_mut(2) {
            let sample = graph.process_sample();
            frame[0] = sample.left;
            frame[1] = sample.right;
        }

        Ok(output.len() / 2)
    }

    /// Sets a runtime control on a module.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let controls = self.control_surfaces.lock().unwrap();
        let control_surface = controls
            .get(module_id)
            .ok_or_else(|| format!("unknown module: {}", module_id))?;
        control_surface.set_control(key, value)?;
        Ok(())
    }

    /// Gets a runtime control on a module.
    pub fn get_control(
        &self,
        module_id: &str,
        key: &str,
    ) -> Result<ControlValue, Box<dyn std::error::Error>> {
        let controls = self.control_surfaces.lock().unwrap();
        let control_surface = controls
            .get(module_id)
            .ok_or_else(|| format!("unknown module: {}", module_id))?;
        Ok(control_surface.get_control(key)?)
    }

    pub fn status(&self) -> RuntimeStatus {
        self.snapshot().status()
    }

    pub fn list_modules(&self) -> Vec<RuntimeModuleInfo> {
        self.snapshot().list_modules()
    }

    pub fn list_connections(&self) -> Vec<RuntimeConnectionInfo> {
        self.snapshot().list_connections()
    }

    pub fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<crate::ControlMeta>)>, GraphCommandError> {
        self.snapshot().list_controls(module_id)
    }

    pub fn add_module(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?
            .clone();

        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }
        let result = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        if let Some(control_surface) = result.control_surface {
            self.control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.to_string(), control_surface);
        }

        self.module_ports.lock().unwrap().insert(
            module_id.to_string(),
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

        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::AddModule {
                module_id: module_id.to_string(),
                module: result.module,
            });

        self.state.lock().unwrap().modules.insert(
            module_id.to_string(),
            RuntimeModuleInfo {
                id: module_id.to_string(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
        );

        if module_type == "code" {
            self.scripts.start_module(
                self.controller().expect("render controller available"),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }
        if module_type == "agent" {
            self.agents.start_module(
                self.controller().expect("render controller available"),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }

        Ok(())
    }

    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        self.scripts.stop_module(module_id);
        self.agents.stop_module(module_id);
        self.control_surfaces
            .lock()
            .unwrap()
            .shift_remove(module_id);
        self.module_ports.lock().unwrap().shift_remove(module_id);
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::RemoveModule {
                module_id: module_id.to_string(),
            });
        let mut state = self.state.lock().unwrap();
        state.modules.shift_remove(module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        Ok(())
    }

    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        {
            let graph = graph.lock().unwrap();
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
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::AddConnection {
                from_module: from_module.to_string(),
                from_port: from_port.to_string(),
                to_module: to_module.to_string(),
                to_port: to_port.to_string(),
            });
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

    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::RemoveConnection {
                from_module: from_module.to_string(),
                from_port: from_port.to_string(),
                to_module: to_module.to_string(),
                to_port: to_port.to_string(),
            });
        self.state.lock().unwrap().connections.retain(|conn| {
            !(conn.from == from_module
                && conn.from_port == from_port
                && conn.to == to_module
                && conn.to_port == to_port)
        });
        Ok(())
    }

    fn install_runtime(&mut self, runtime: super::runtime::InventionRuntime) {
        self.scripts.stop_all();
        self.agents.stop_all();
        let mut input_map: std::collections::HashMap<String, Vec<RoutingConnection>> =
            std::collections::HashMap::new();

        for conn in &runtime.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_default()
                .push(conn.clone());
        }

        let (_, command_rx) = mpsc::channel();

        runtime.state.lock().unwrap().running = true;

        *self.module_ports.lock().unwrap() = module_ports(&runtime.modules);
        self.graph = Some(Arc::new(Mutex::new(SignalGraph {
            modules: runtime.modules,
            sinks: runtime.sinks,
            input_map,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            topo_dirty: true,
        })));
        self.registry = runtime.registry;
        self.state = runtime.state;
        *self.control_surfaces.lock().unwrap() = runtime.control_surfaces;
        if let Some(controller) = self.controller() {
            self.scripts.start_all(controller.clone());
            self.agents.start_all(controller);
        }
    }
}

impl OrchestrationRuntime for RenderEngine {
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
    ) -> Result<Vec<(String, Vec<crate::ControlMeta>)>, GraphCommandError> {
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
    use super::{CodeModuleRuntimeInfo, RenderEngine};
    use crate::ControlValue;
    use std::time::Duration;

    const SIMPLE_INVENTION: &str = r#"{
        "version": "1.0.0",
        "title": "render-test",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
            { "id": "vca", "type": "vca", "config": { "level": 0.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
            { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;

    #[test]
    fn render_engine_renders_interleaved_audio() {
        let mut engine = RenderEngine::new(48_000);
        engine.load_json(SIMPLE_INVENTION).unwrap();
        engine
            .set_control("vca", "cv", ControlValue::Number(0.5))
            .unwrap();

        let mut output = [0.0f32; 16];
        let frames = engine.render_interleaved(&mut output).unwrap();

        assert_eq!(frames, 8);
        assert!(output.iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn render_engine_reset_restores_state() {
        let mut engine = RenderEngine::new(48_000);
        engine.load_json(SIMPLE_INVENTION).unwrap();
        engine
            .set_control("vca", "cv", ControlValue::Number(0.0))
            .unwrap();

        let mut silent = [0.0f32; 8];
        engine.render_interleaved(&mut silent).unwrap();

        engine
            .set_control("vca", "cv", ControlValue::Number(0.8))
            .unwrap();
        engine.reset().unwrap();

        let level = engine.get_control("vca", "cv").unwrap();
        assert_eq!(level, ControlValue::Number(1.0));
    }

    #[test]
    fn render_engine_supports_runtime_graph_mutation() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        engine
            .add_module(
                "osc",
                "oscillator",
                &serde_json::json!({ "waveform": "sine", "frequency": 440.0 }),
            )
            .unwrap();
        engine
            .add_module("vca", "vca", &serde_json::json!({ "level": 0.0 }))
            .unwrap();
        engine.connect("osc", "audio", "vca", "audio").unwrap();
        engine.connect("vca", "audio", "dac", "audio").unwrap();
        engine
            .set_control("vca", "cv", ControlValue::Number(0.5))
            .unwrap();

        assert_eq!(engine.list_modules().len(), 3);
        assert_eq!(engine.list_connections().len(), 2);

        let mut output = [0.0f32; 16];
        engine.render_interleaved(&mut output).unwrap();
        assert!(output.iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn render_engine_runs_code_module_init_hook() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "function init() { graph.addModule('osc_from_code', 'oscillator', { waveform: 'sine', frequency: 330.0 }) }"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(50));
        assert!(engine
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_code"));
    }

    #[test]
    fn render_engine_lists_code_module_runtime_info() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "function init() {}",
                            "entrypoint": "init",
                            "enabled": true,
                            "tick_hz": 8.0
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        let modules = engine.list_code_modules().unwrap();
        assert_eq!(modules.len(), 1);
        assert!(matches!(
            &modules[0],
            CodeModuleRuntimeInfo {
                id,
                entrypoint,
                enabled,
                tick_hz,
                ..
            } if id == "code1" && entrypoint == "init" && *enabled && (*tick_hz - 8.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn render_engine_supports_returned_lifecycle_object() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "(() => ({ init() { graph.addModule('osc_from_object', 'oscillator', { waveform: 'sine', frequency: 440.0 }) } }))()"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(50));
        assert!(engine
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_object"));
    }

    #[test]
    fn render_engine_supports_custom_entrypoint_function() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "entrypoint": "boot",
                            "script": "function boot() { graph.addModule('osc_from_boot', 'oscillator', { waveform: 'sine', frequency: 660.0 }) }"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(50));
        assert!(engine
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_boot"));
    }

    #[test]
    fn render_engine_keeps_legacy_globalthis_hooks_working() {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    {
                        "id": "code1",
                        "type": "code",
                        "config": {
                            "script": "globalThis.init = function () { graph.addModule('osc_from_legacy', 'oscillator', { waveform: 'sine', frequency: 550.0 }) }"
                        }
                    },
                    { "id": "dac", "type": "dac" }
                ],
                "connections": []
            }"#,
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(50));
        assert!(engine
            .list_modules()
            .into_iter()
            .any(|module| module.id == "osc_from_legacy"));
    }
}
