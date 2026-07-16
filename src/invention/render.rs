//! Offline invention renderer for host-driven playback.

use indexmap::IndexMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::agents::AgentManager;
use crate::scripting::ScriptManager;
use crate::{ControlValue, Invention, InventionBuilder, InventionHandles, ModuleRegistry};

use super::graph::{GraphCommand, SignalGraph};
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
    handles: Arc<Mutex<InventionHandles>>,
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
            handles: Arc::new(Mutex::new(InventionHandles::empty())),
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

    pub fn full_snapshot(&self) -> crate::RuntimeFullSnapshot {
        let module_ports = self.module_ports.lock().unwrap();
        self.snapshot().full_snapshot_with_ports(&module_ports)
    }

    /// Returns the declarative document describing the current graph: the
    /// authored document as loaded, updated by runtime mutations, with
    /// connections mirrored from the live topology.
    pub fn document(&self) -> Option<Invention> {
        self.snapshot().document()
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

    /// Loads an invention from a parsed value.
    pub fn load_invention(
        &mut self,
        invention: Invention,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let builder = InventionBuilder::new(self.sample_rate);
        let (runtime, handles) = builder.build(invention)?;
        *self.handles.lock().unwrap() = handles;
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
        if !output.len().is_multiple_of(2) {
            return Err("output buffer length must be even".into());
        }

        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| "no invention loaded".to_string())?;
        let mut graph = graph.lock().unwrap();

        let frames_total = output.len() / 2;
        let block = graph.block_size.clamp(1, crate::MAX_BLOCK);
        let mut left = [0.0f32; crate::MAX_BLOCK];
        let mut right = [0.0f32; crate::MAX_BLOCK];

        let mut done = 0;
        while done < frames_total {
            let n = (frames_total - done).min(block);
            graph.process_block(&mut left[..n], &mut right[..n]);
            for k in 0..n {
                output[(done + k) * 2] = left[k];
                output[(done + k) * 2 + 1] = right[k];
            }
            done += n;
        }

        Ok(frames_total)
    }

    /// Sets the audio processing block size in frames (clamped to
    /// `[1, MAX_BLOCK]`). Larger blocks amortize per-call overhead; smaller
    /// blocks reduce control/feedback latency. Defaults to
    /// [`crate::DEFAULT_BLOCK_SIZE`].
    pub fn set_block_size(&self, block_size: usize) {
        if let Some(graph) = self.graph.as_ref() {
            graph.lock().unwrap().set_block_size(block_size);
        }
    }

    /// Returns the current audio processing block size in frames.
    pub fn block_size(&self) -> usize {
        self.graph
            .as_ref()
            .map(|graph| graph.lock().unwrap().block_size)
            .unwrap_or(crate::DEFAULT_BLOCK_SIZE)
    }

    /// Scans the most recently rendered block for a rising `end` gate and
    /// returns the frame index (within that block) where the piece ended.
    ///
    /// `source` names the module whose `end` output is authoritative; when
    /// `None`, every module exposing an `end` output is watched and the
    /// earliest high frame wins ("the piece ends when any end gate fires" —
    /// name a source for multi-lane pieces with uneven lanes). `frames` is
    /// the number of valid frames in the last render call, which must not
    /// have exceeded one graph block for the scan to be frame-exact (the
    /// `end` gate is latched, so a coarser host still cannot *miss* it —
    /// only land on a later frame).
    ///
    /// Errors when no invention is loaded, when a named source does not
    /// exist or has no `end` output, or when `source` is `None` and nothing
    /// in the graph exposes an `end` output (the render would never stop).
    pub fn scan_end_gate(
        &self,
        source: Option<&str>,
        frames: usize,
    ) -> Result<Option<usize>, Box<dyn std::error::Error>> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| "no invention loaded".to_string())?;
        let graph = graph.lock().unwrap();

        let mut earliest: Option<usize> = None;
        let mut candidates = 0usize;
        for (id, module) in graph.modules.iter() {
            if let Some(wanted) = source {
                if id != wanted {
                    continue;
                }
            }
            let module = module.module();
            let Some(port) = module.outputs().iter().position(|port| *port == "end") else {
                if source.is_some() {
                    return Err(format!("module '{}' has no 'end' output", id).into());
                }
                continue;
            };
            candidates += 1;
            let block = module.output_block(port);
            let n = frames.min(block.len());
            if let Some(frame) = block[..n].iter().position(|&value| value > 0.5) {
                earliest = Some(earliest.map_or(frame, |current| current.min(frame)));
            }
        }

        if candidates == 0 {
            return Err(match source {
                Some(wanted) => format!("unknown end source module '{}'", wanted).into(),
                None => "no module exposes an 'end' output; the render would never stop \
                     (use a one_shot sequencer or an explicit duration)"
                    .to_string()
                    .into(),
            });
        }

        Ok(earliest)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn finish_audio_file_sink(
        &self,
        module_id: &str,
    ) -> Result<crate::AudioFileSinkStats, Box<dyn std::error::Error>> {
        let handle = self.audio_file_sink_handle(module_id)?;
        Ok(handle.finish())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn audio_file_sink_wav_bytes(
        &self,
        module_id: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let handle = self.audio_file_sink_handle(module_id)?;
        handle.wav_bytes().map_err(Into::into)
    }

    #[cfg(target_arch = "wasm32")]
    fn audio_file_sink_handle(
        &self,
        module_id: &str,
    ) -> Result<crate::AudioFileSinkHandle, Box<dyn std::error::Error>> {
        let key = format!("{}.handle", module_id);
        self.handles
            .lock()
            .unwrap()
            .get::<crate::AudioFileSinkHandle>(&key)
            .ok_or_else(|| format!("unknown audio_file_sink handle: {}", module_id).into())
    }

    /// Returns whether a one-shot playthrough has ended, observed via module
    /// controls (the same surface live playback uses; `scan_end_gate` is the
    /// frame-exact alternative for offline rendering).
    pub fn end_reached(&self, source: Option<&str>) -> Result<bool, String> {
        let surfaces = self.control_surfaces.lock().unwrap();
        super::runtime::end_reached_in(&surfaces, source)
    }

    /// Sets a runtime control on a module.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Invoke outside the directory lock: a scheduler's `schedule` write
        // re-resolves its targets against this same directory.
        let control_surface = {
            let controls = self.control_surfaces.lock().unwrap();
            controls
                .get(module_id)
                .cloned()
                .ok_or_else(|| format!("unknown module: {}", module_id))?
        };
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

        let mut new_handles = std::collections::HashMap::new();
        for (handle_name, handle) in result.handles {
            let key = format!("{}.{}", module_id, handle_name);
            new_handles.insert(key, handle);
        }

        // Attach schedulers before touching the graph, so a schedule that
        // fails to resolve leaves the loaded invention unchanged.
        if module_type == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            crate::modules::control_scheduler::attach_from_handle(
                module_id,
                new_handles.get(&format!("{}.controls", module_id)),
                &self.control_surfaces,
            )
            .map_err(GraphCommandError::ModuleBuildFailed)?;
        }

        self.handles
            .lock()
            .unwrap()
            .merge(InventionHandles::new(new_handles));

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

        {
            let mut state = self.state.lock().unwrap();
            state.modules.insert(
                module_id.to_string(),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
            state.document_upsert_module(module_id, module_type, config);
        }

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
        self.handles
            .lock()
            .unwrap()
            .remove_prefix(&format!("{}.", module_id));
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
        state.document_remove_module(module_id);
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
        let (_, command_rx) = mpsc::channel();

        runtime.state.lock().unwrap().running = true;

        *self.module_ports.lock().unwrap() = module_ports(&runtime.modules);
        self.graph = Some(Arc::new(Mutex::new(SignalGraph {
            modules: runtime.modules,
            sinks: runtime.sinks,
            edges: runtime.routing,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            compiled_routes: Vec::new(),
            connected_in_ports: Vec::new(),
            process_groups: Vec::new(),
            sink_indices: Vec::new(),
            out_bufs: Vec::new(),
            out_prev: Vec::new(),
            out_counts: Vec::new(),
            block_capacity: 0,
            block_size: crate::DEFAULT_BLOCK_SIZE,
            topo_dirty: true,
        })));
        self.registry = runtime.registry;
        self.state = runtime.state;
        // Adopt the runtime's directory (rather than copying its contents)
        // so schedulers attached at build time keep resolving against the
        // live map. Stale snapshots of the previous invention keep the old
        // directory, matching how `state` is replaced above.
        self.control_surfaces = runtime.control_surfaces;
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
mod tests;
