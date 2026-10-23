//! Offline invention renderer for host-driven playback.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use crate::agents::AgentManager;
use crate::scripting::ScriptManager;
use crate::{ControlValue, Invention, InventionBuilder, InventionHandles, ModuleRegistry};

use super::graph::ended::is_end_output;
use super::graph::SignalGraph;
use super::orchestration::{ModulePorts, OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
use super::runtime::{module_ports, ControlSurfaceInstance, GraphCommandError};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};

#[cfg(target_arch = "wasm32")]
mod audio_file_sink;
mod code_modules;
mod editing;
mod replay;

pub use code_modules::CodeModuleRuntimeInfo;

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
    /// Offline render has no event consumer, so this slot stays empty; it exists
    /// only so snapshots share the [`RuntimeSnapshot`] shape with live runtimes.
    event_sink: super::orchestration::EventSinkSlot,
    /// A live session's requests still to apply (see [`Self::replay`]).
    replay: replay::Replay,
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
            event_sink: Arc::new(Mutex::new(None)),
            replay: replay::Replay::default(),
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
            event_sink: self.event_sink.clone(),
            live: None,
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
            registry: Some(self.registry.clone()),
            sample_rate: self.sample_rate,
            graph: Some(self.graph.as_ref()?.clone()),
            live: None,
            module_ports: self.module_ports.clone(),
        })
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

    /// Replays a live session's request log (see
    /// [`RunningInvention::request_log`](crate::RunningInvention::request_log)):
    /// from now on, renders apply each logged request at its recorded
    /// sample, splitting the render there as the live block was split.
    ///
    /// Load the invention the session played, as loaded, and replay before
    /// rendering, at the session's sample rate and block size: the render
    /// then matches what played sample for sample (see
    /// [`RequestLog::is_exact`](crate::RequestLog::is_exact) for what it
    /// cannot reproduce). Replay is by sample, not by beat. Replaces any
    /// replay in progress; loading an invention ends it. Fails, replaying
    /// nothing, when the log was played at another sample rate or block
    /// size, is out of time order, or names a control not declared here.
    pub fn replay(&mut self, log: &crate::RequestLog) -> Result<(), Box<dyn std::error::Error>> {
        self.replay = replay::Replay::default();
        let block_size = self.block_size();
        if (log.sample_rate, log.block_size) != (self.sample_rate, block_size) {
            return Err(format!(
                "the log was played at {} Hz in blocks of {}; this render is {} Hz in blocks of {}",
                log.sample_rate, log.block_size, self.sample_rate, block_size
            )
            .into());
        }
        let replay = {
            let surfaces = self.control_surfaces.lock().unwrap();
            replay::Replay::resolve(log, &surfaces)?
        };
        if let Some(graph) = &self.graph {
            replay.check_clocks(&mut graph.lock().unwrap())?;
        }
        self.replay = replay;
        Ok(())
    }

    /// How many replayed requests the render refused so far (a value its
    /// control does not hold, a module gone): none, for a log of this
    /// invention.
    pub fn replay_refused(&self) -> usize {
        self.replay.refused
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
            if let Some(watch) = &mut graph.end_watch {
                watch.begin();
            }
            // Split where a replayed request is due, as the live drain does.
            let mut at = 0;
            while at < n {
                let end = at + self.replay.apply_due(&mut graph, n - at);
                graph.process_block(&mut left[at..end], &mut right[at..end]);
                at = end;
            }
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

    /// Scans the most recently rendered block for a rising `ended` gate and
    /// returns the frame index (within that block) where the piece ended.
    ///
    /// `source` names the module whose `ended` output is authoritative; when
    /// `None`, every module exposing an `ended` output is watched and the
    /// earliest high frame wins ("the piece ends when any end gate fires" —
    /// name a source for multi-lane pieces with uneven lanes). `frames` is
    /// the number of valid frames in the last render call, which must not
    /// have exceeded one graph block for the scan to be frame-exact (the
    /// `ended` gate is latched, so a coarser host still cannot *miss* it —
    /// only land on a later frame). A block that processed in pieces (split
    /// by a replayed request, see [`Self::replay`], or by the graph at a
    /// scheduler's entry) is answered for whole, and a module another edit
    /// moved since the render still answers for what it rendered.
    ///
    /// Errors when no invention is loaded, when a named source does not
    /// exist or has no `ended` output, or when `source` is `None` and nothing
    /// in the graph exposes an `ended` output (the render would never stop).
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
            let Some(port) = module.outputs().iter().position(|port| is_end_output(port)) else {
                if source.is_some() {
                    return Err(format!("module '{}' has no 'ended' output", id).into());
                }
                continue;
            };
            candidates += 1;
            // The watch saw every piece of the block; a module's buffers keep
            // only the last.
            let watched = graph.end_watch.as_ref().and_then(|watch| watch.rise(id));
            let rise = watched.unwrap_or_else(|| {
                let block = module.output_block(port);
                let n = frames.min(block.len());
                block[..n].iter().position(|&value| value > 0.5)
            });
            if let Some(frame) = rise.filter(|&frame| frame < frames) {
                earliest = Some(earliest.map_or(frame, |current| current.min(frame)));
            }
        }

        if candidates == 0 {
            return Err(match source {
                Some(wanted) => format!("unknown end source module '{}'", wanted).into(),
                None => "no module exposes an 'ended' output; the render would never stop \
                     (use a one_shot sequencer or an explicit duration)"
                    .to_string()
                    .into(),
            });
        }

        Ok(earliest)
    }

    /// Returns whether a one-shot playthrough has ended, observed via module
    /// controls (the same surface live playback uses; `scan_end_gate` is the
    /// frame-exact alternative for offline rendering).
    pub fn end_reached(&self, source: Option<&str>) -> Result<bool, String> {
        let surfaces = self.control_surfaces.lock().unwrap();
        super::runtime::end_reached_in(&surfaces, source)
    }

    /// Sets a runtime control on a module, coercing it to the control's
    /// declared kind and refusing a number that is not finite.
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
        // Coerce to the control's declared kind so a stringified write lands,
        // matching the live runtime's behavior (see FUG-240). Then refuse a
        // number that is not finite, as the live runtime does.
        let value = control_surface.coerce_value(key, value);
        if let Some(message) = crate::rpc::non_finite_refusal(module_id, key, &value) {
            return Err(message.into());
        }
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

    fn install_runtime(&mut self, runtime: super::runtime::InventionRuntime) {
        self.replay = replay::Replay::default();
        self.scripts.stop_all();
        self.agents.stop_all();
        runtime.state.lock().unwrap().running = true;

        if let Some(previous) = &self.graph {
            super::declared::retire_offline(previous, &self.control_surfaces);
        }
        *self.module_ports.lock().unwrap() = module_ports(&runtime.modules);
        let mut graph = SignalGraph::new(
            runtime.modules,
            runtime.sinks,
            runtime.routing,
            // Offline render has no sampler: a meter nobody drains, and no
            // spectrum ring.
            super::graph::MasterObservers::default(),
        );
        graph.end_watch = Some(Default::default());
        // No request reaches it, but it plays the modules' tables.
        super::graph::offline_requests(&mut graph);
        let graph = Arc::new(Mutex::new(graph));
        super::declared::bind_offline(&graph, &runtime.control_surfaces);
        self.graph = Some(graph);
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
mod edit_state_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod end_tests;
#[cfg(test)]
mod retarget_tests;
#[cfg(test)]
mod tests;
