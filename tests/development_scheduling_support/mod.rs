//! Helpers for the development scheduling acceptance tests: documents that
//! wrap a development definition or place the same wiring flat in the main
//! graph, offline and live renderers, and a pass-through probe module that
//! records how many frames each `process` call was given.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fugue::{
    AudioBackend, GraphModule, Invention, InventionBuilder, Module, ModuleBuildResult,
    ModuleFactory, ModuleRegistry, MAX_BLOCK,
};
use serde_json::{json, Value};

pub const SAMPLE_RATE: u32 = 48_000;

// ---------------------------------------------------------------------------
// Documents

/// The development type name the wrapped documents register `definition` as.
pub const DEV_TYPE: &str = "scheduled_voice";
/// The module id of the development instance in the wrapped documents.
pub const DEV_ID: &str = "voice";

/// A main graph holding the development instance `voice` of type
/// `scheduled_voice`. `modules` lists the main graph's own modules in order,
/// with the placeholder `{"id":"voice"}` where the instance sits.
pub fn wrapped(definition: &Value, modules: &[Value], connections: &[Value]) -> Invention {
    let modules: Vec<Value> = modules
        .iter()
        .map(|m| {
            if m["id"] == DEV_ID {
                json!({ "id": DEV_ID, "type": DEV_TYPE, "config": {} })
            } else {
                m.clone()
            }
        })
        .collect();
    let doc = json!({
        "version": "1.0.0",
        "developments": [ { "name": DEV_TYPE, "definition": definition } ],
        "modules": modules,
        "connections": connections,
    });
    serde_json::from_value(doc).unwrap()
}

/// The same wiring placed flat in the main graph: the instance's inner
/// modules take its place in the module list, the inner connections follow the
/// main ones, and every main connection into or out of the instance is
/// rewritten onto the inner port its input or output name maps to.
pub fn flat(definition: &Value, modules: &[Value], connections: &[Value]) -> Invention {
    let inputs = definition["inputs"].as_array().cloned().unwrap_or_default();
    let outputs = definition["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut flat_modules = Vec::new();
    for m in modules {
        if m["id"] == DEV_ID {
            flat_modules.extend(definition["modules"].as_array().unwrap().iter().cloned());
        } else {
            flat_modules.push(m.clone());
        }
    }
    let mut flat_connections = Vec::new();
    for c in connections {
        let mut c = c.clone();
        if c["to"] == DEV_ID {
            let input = inputs.iter().find(|i| i["name"] == c["to_port"]).unwrap();
            c["to"] = input["to"].clone();
            c["to_port"] = input["to_port"].clone();
        }
        if c["from"] == DEV_ID {
            let output = outputs
                .iter()
                .find(|o| o["name"] == c["from_port"])
                .unwrap();
            c["from"] = output["from"].clone();
            c["from_port"] = output["from_port"].clone();
        }
        flat_connections.push(c);
    }
    flat_connections.extend(
        definition["connections"]
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    let doc = json!({
        "version": "1.0.0",
        "modules": flat_modules,
        "connections": flat_connections,
    });
    serde_json::from_value(doc).unwrap()
}

// ---------------------------------------------------------------------------
// Rendering

/// Renders `frames` stereo frames offline at `block`.
pub fn offline(doc: Invention, frames: usize, block: usize) -> Vec<f32> {
    let mut engine = fugue::RenderEngine::new(SAMPLE_RATE);
    engine.load_invention(doc).unwrap();
    engine.set_block_size(block);
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// A backend that hands its render callback to the test.
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

/// Plays `doc` live through a capture backend for `blocks` callbacks of
/// `block` frames each (`block <= 64`), interleaved stereo.
pub fn live(registry: ModuleRegistry, doc: Invention, blocks: usize, block: usize) -> Vec<f32> {
    assert!(block <= 64);
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry)
        .build(doc)
        .unwrap();
    let slot = Arc::new(Mutex::new(None));
    let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
    let mut out = Vec::with_capacity(blocks * block * 2);
    {
        let mut slot = slot.lock().unwrap();
        let render = slot.as_mut().unwrap();
        let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
        for _ in 0..blocks {
            render(&mut left[..block], &mut right[..block]);
            for i in 0..block {
                out.extend([left[i], right[i]]);
            }
        }
    }
    running.stop();
    out
}

/// Asserts two renders are bit-for-bit identical.
pub fn assert_identical(a: &[f32], b: &[f32], label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: length mismatch");
    let first = a
        .iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits());
    if let Some(i) = first {
        panic!(
            "{label}: first difference at sample {i} ({} vs {})",
            a[i], b[i]
        );
    }
}

// ---------------------------------------------------------------------------
// Probe module

/// What a probe saw: how many `process` calls, and the frames each was given.
pub struct ProbeStats {
    pub calls: AtomicUsize,
    pub max_frames: AtomicUsize,
    pub min_frames: AtomicUsize,
}

impl ProbeStats {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            max_frames: AtomicUsize::new(0),
            min_frames: AtomicUsize::new(usize::MAX),
        })
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn max_frames(&self) -> usize {
        self.max_frames.load(Ordering::SeqCst)
    }
    pub fn min_frames(&self) -> usize {
        self.min_frames.load(Ordering::SeqCst)
    }
}

/// Passes `audio` through unchanged and records the size of each call.
struct Probe {
    stats: Arc<ProbeStats>,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Module for Probe {
    fn name(&self) -> &str {
        "Probe"
    }
    fn process(&mut self, frames: usize) -> bool {
        self.stats.calls.fetch_add(1, Ordering::SeqCst);
        self.stats.max_frames.fetch_max(frames, Ordering::SeqCst);
        self.stats.min_frames.fetch_min(frames, Ordering::SeqCst);
        self.output[..frames].copy_from_slice(&self.input[..frames]);
        true
    }
    fn inputs(&self) -> &[&str] {
        &["audio"]
    }
    fn outputs(&self) -> &[&str] {
        &["audio"]
    }
    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.input
    }
    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.input.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {port}")),
        }
    }
    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.output[0]),
            _ => Err(format!("Unknown output port: {port}")),
        }
    }
}

struct ProbeFactory {
    type_id: &'static str,
    stats: Arc<ProbeStats>,
}

impl ModuleFactory for ProbeFactory {
    fn type_id(&self) -> &'static str {
        self.type_id
    }
    fn input_ports(&self) -> Option<&'static [&'static str]> {
        Some(&["audio"])
    }
    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&["audio"])
    }
    fn build(
        &self,
        _sample_rate: u32,
        _config: &Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let probe = Probe {
            stats: self.stats.clone(),
            input: vec![0.0; MAX_BLOCK],
            output: vec![0.0; MAX_BLOCK],
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(probe)),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

/// The default registry plus one probe module type per name, each with its
/// own stats, returned in the order given.
pub fn registry_with_probes(names: &[&'static str]) -> (ModuleRegistry, Vec<Arc<ProbeStats>>) {
    let mut registry = ModuleRegistry::default();
    let mut all = Vec::new();
    for &type_id in names {
        let stats = ProbeStats::new();
        registry.register(ProbeFactory {
            type_id,
            stats: stats.clone(),
        });
        all.push(stats);
    }
    (registry, all)
}
