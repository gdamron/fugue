//! Offline invention renderer for host-driven playback.

use indexmap::IndexMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::{ControlValue, Invention, InventionBuilder};

use super::graph::{RoutingConnection, SignalGraph};
use super::runtime::ControlSurfaceInstance;

/// Offline renderer for inventions.
///
/// Unlike [`super::runtime::RunningInvention`], this type does not own an audio
/// device. Hosts drive rendering explicitly by providing their own output
/// buffers, which makes the engine suitable for FFI and wasm consumers.
pub struct RenderEngine {
    sample_rate: u32,
    graph: Option<SignalGraph>,
    control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    source_json: Option<String>,
}

impl RenderEngine {
    /// Creates a new renderer with the provided sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            graph: None,
            control_surfaces: Arc::new(Mutex::new(IndexMap::new())),
            source_json: None,
        }
    }

    /// Returns the configured sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
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
            .as_mut()
            .ok_or_else(|| "no invention loaded".to_string())?;

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

    fn install_runtime(&mut self, runtime: super::runtime::InventionRuntime) {
        let mut input_map: std::collections::HashMap<String, Vec<RoutingConnection>> =
            std::collections::HashMap::new();

        for conn in &runtime.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_default()
                .push(conn.clone());
        }

        let (_, command_rx) = mpsc::channel();

        self.graph = Some(SignalGraph {
            modules: runtime.modules,
            sinks: runtime.sinks,
            input_map,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            topo_dirty: true,
        });
        *self.control_surfaces.lock().unwrap() = runtime.control_surfaces;
    }
}

#[cfg(test)]
mod tests {
    use super::RenderEngine;
    use crate::ControlValue;

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
}
