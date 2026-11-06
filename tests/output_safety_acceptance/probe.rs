//! A scripted source module and a live harness around it.
//!
//! The source emits a repeating pattern on each of its three outputs
//! (`audio`, `audio_left`, `audio_right`), configured as arrays of numbers
//! or the strings "nan", "inf", "-inf". Inside every `process` call it
//! records whether flush-to-zero was on and what a subnormal times 0.5
//! (a simple decaying path) came to.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fugue::module_config::ConfigKey;
use fugue::{
    AudioBackend, GraphModule, Invention, InventionBuilder, Module, ModuleBuildResult,
    ModuleFactory, ModuleRegistry, RunningInvention, MAX_BLOCK,
};

use super::*;

pub const SAMPLE_RATE: u32 = 48_000;
const OUTPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

#[derive(Default)]
pub struct Log {
    pub calls: AtomicUsize,
    pub flag_missing: AtomicBool,
    pub decay_left_nonzero: AtomicBool,
}

struct Source {
    log: Arc<Log>,
    patterns: [Vec<f32>; 3],
    pos: usize,
    blocks: [Vec<f32>; 3],
}

impl Module for Source {
    fn name(&self) -> &str {
        "Source"
    }
    fn process(&mut self, frames: usize) -> bool {
        self.log.calls.fetch_add(1, Ordering::SeqCst);
        if !fugue::flush_to_zero_enabled() {
            self.log.flag_missing.store(true, Ordering::SeqCst);
        }
        let decayed = std::hint::black_box(subnormal()) * std::hint::black_box(0.5f32);
        if decayed != 0.0 {
            self.log.decay_left_nonzero.store(true, Ordering::SeqCst);
        }
        for (block, pattern) in self.blocks.iter_mut().zip(&self.patterns) {
            for (i, out) in block[..frames].iter_mut().enumerate() {
                *out = match pattern.len() {
                    0 => 0.0,
                    n => pattern[(self.pos + i) % n],
                };
            }
        }
        self.pos += frames;
        true
    }
    fn inputs(&self) -> &[&str] {
        &[]
    }
    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }
    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }
    fn output_block(&self, index: usize) -> &[f32] {
        &self.blocks[index]
    }
    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input {port}"))
    }
    fn get_output(&self, port: &str) -> Result<f32, String> {
        let i = OUTPUTS
            .iter()
            .position(|p| *p == port)
            .ok_or(port.to_string())?;
        Ok(self.blocks[i][0])
    }
}

struct SourceFactory(Arc<Log>);

fn parse(v: &Value) -> f32 {
    match v.as_str() {
        Some("nan") => f32::NAN,
        Some("inf") => f32::INFINITY,
        Some("-inf") => f32::NEG_INFINITY,
        _ => v.as_f64().expect("number") as f32,
    }
}

impl ModuleFactory for SourceFactory {
    fn type_id(&self) -> &'static str {
        "scripted_source"
    }
    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                ConfigKey::json("audio"),
                ConfigKey::json("audio_left"),
                ConfigKey::json("audio_right"),
            ]
        }
    }
    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&OUTPUTS)
    }
    fn build(
        &self,
        _sample_rate: u32,
        config: &Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let pattern = |key: &str| -> Vec<f32> {
            let list = config.get(key).and_then(|v| v.as_array());
            list.map(|a| a.iter().map(parse).collect())
                .unwrap_or_default()
        };
        let source = Source {
            log: self.0.clone(),
            patterns: [
                pattern("audio"),
                pattern("audio_left"),
                pattern("audio_right"),
            ],
            pos: 0,
            blocks: std::array::from_fn(|_| vec![0.0; MAX_BLOCK]),
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(source)),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

/// A scripted source wired to a dac, playing through a capturing backend.
pub struct Live {
    pub running: RunningInvention,
    pub log: Arc<Log>,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Live {
    /// `patterns` is the source's config: arrays under `audio`, `audio_left`,
    /// `audio_right`.
    pub fn start(patterns: Value) -> Live {
        let log = Arc::new(Log::default());
        let mut registry = ModuleRegistry::default();
        registry.register(SourceFactory(log.clone()));
        let doc: Invention = serde_json::from_value(json!({
            "version": "1.0.0",
            "modules": [
                { "id": "src", "type": "scripted_source", "config": patterns },
                { "id": "dac", "type": "dac", "config": {} },
            ],
            "connections": OUTPUTS.iter().map(|p| json!(
                { "from": "src", "from_port": p, "to": "dac", "to_port": p }
            )).collect::<Vec<_>>(),
        }))
        .unwrap();
        let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry)
            .build(doc)
            .unwrap();
        let slot = Arc::new(Mutex::new(None));
        let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
        Live { running, log, slot }
    }

    /// One callback of `frames` frames (at most 64): the (left, right) it filled.
    pub fn callback(&self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        assert!(frames <= 64);
        let (mut l, mut r) = ([f32::MAX; 64], [f32::MAX; 64]);
        let mut slot = self.slot.lock().unwrap();
        (slot.as_mut().expect("backend started"))(&mut l[..frames], &mut r[..frames]);
        (l[..frames].to_vec(), r[..frames].to_vec())
    }

    /// `frames` frames in callbacks of 64.
    pub fn render(&self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for _ in 0..frames / 64 {
            let (bl, br) = self.callback(64);
            l.extend(bl);
            r.extend(br);
        }
        (l, r)
    }
}
