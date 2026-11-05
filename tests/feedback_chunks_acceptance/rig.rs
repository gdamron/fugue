//! Shared rig: invention documents, offline and live rendering, signal
//! metrics. Self-contained (no `super`), so the kill-switch binary can
//! include this file with `#[path]`.
#![allow(dead_code)]

use fugue::{
    AudioBackend, GraphModule, Invention, InventionBuilder, ModuleRegistry, RenderEngine,
    RunningInvention,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub const SR: u32 = 48_000;
/// The live pump's block, in frames (the largest a backend callback may be
/// given for these tests to stay frame-exact).
pub const BLOCK: usize = 64;
/// Level an echo's rising edge must cross to count as heard.
pub const EDGE: f32 = 0.02;

// -- documents ---------------------------------------------------------------

pub fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

pub fn module(id: &str, kind: &str, config: Value) -> Value {
    json!({ "id": id, "type": kind, "config": config })
}

/// A document under construction.
#[derive(Clone)]
pub struct Doc {
    pub modules: Vec<Value>,
    pub connections: Vec<Value>,
    pub developments: Vec<Value>,
}

impl Doc {
    pub fn new() -> Self {
        Doc {
            modules: vec![],
            connections: vec![],
            developments: vec![],
        }
    }
    pub fn add(mut self, id: &str, kind: &str, config: Value) -> Self {
        self.modules.push(module(id, kind, config));
        self
    }
    pub fn wire(mut self, from: &str, from_port: &str, to: &str, to_port: &str) -> Self {
        self.connections.push(conn(from, from_port, to, to_port));
        self
    }
    /// Drops the connection(s) `from.from_port -> to.to_port`.
    pub fn unwire(mut self, from: &str, from_port: &str, to: &str, to_port: &str) -> Self {
        let gone = conn(from, from_port, to, to_port);
        self.connections.retain(|c| *c != gone);
        self
    }
    pub fn json(&self) -> Value {
        let mut doc = json!({ "version": "1.0.0", "modules": self.modules,
                              "connections": self.connections });
        if !self.developments.is_empty() {
            doc["developments"] = Value::Array(self.developments.clone());
        }
        doc
    }
    pub fn invention(&self) -> Invention {
        serde_json::from_value(self.json()).expect("the document parses")
    }
}

pub fn dac() -> Value {
    module("dac", "dac", json!({ "soft_clip": false }))
}

/// Delay config for a whole-sample `time` (seconds), with an optional `min_time`.
pub fn delay_cfg(time: f64, min_time: Option<f64>) -> Value {
    let mut cfg = json!({ "time": time, "feedback": 0.0 });
    if let Some(min) = min_time {
        cfg["min_time"] = json!(min);
    }
    cfg
}

/// A clock gate (one pulse every `60 / bpm` s, ~64 samples long) scaled to
/// 0.5 as `src`, with `src` also sent to `dac.audio_left` as the dry marker.
pub fn gated(bpm: f64) -> Doc {
    let gate_length = 64.0 / (f64::from(SR) * 60.0 / bpm);
    Doc::new()
        .add(
            "clk",
            "clock",
            json!({ "bpm": bpm, "gate_length": gate_length }),
        )
        .add("src", "vca", json!({ "level": 0.5 }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("clk", "beat", "src", "audio")
        .wire("src", "audio", "dac", "audio_left")
}

/// The gate through one delay `d` with a loop patched outside the atom
/// (`d.audio -> loop -> d.audio`, level `gain`). Left is the dry gate, right
/// the delay's output.
pub fn gate_loop(delay: Value, gain: f64, bpm: f64) -> Doc {
    gated(bpm)
        .add("d", "delay", delay)
        .add("loop", "vca", json!({ "level": gain }))
        .wire("src", "audio", "d", "audio")
        .wire("d", "audio", "loop", "audio")
        .wire("loop", "audio", "d", "audio")
        .wire("d", "audio", "dac", "audio_right")
}

/// `gate_loop` plus a vca `m` (level 0, so it never moves the delay time)
/// listening to the delay but not yet patched into `time_mod`.
pub fn with_mod_tap(doc: Doc) -> Doc {
    doc.add("m", "vca", json!({ "level": 0.0 }))
        .wire("d", "audio", "m", "audio")
}

pub fn patch_mod(doc: Doc) -> Doc {
    doc.wire("m", "audio", "d", "time_mod")
}

// -- developments -----------------------------------------------------------

pub fn definition(modules: Vec<Value>, connections: Vec<Value>, input: (&str, &str)) -> Value {
    json!({ "version": "1.0.0", "modules": modules, "connections": connections,
            "inputs": [ { "name": "audio", "to": input.0, "to_port": input.1 } ],
            "outputs": [ { "name": "audio", "from": "d", "from_port": "audio" } ] })
}

pub fn with_development(mut doc: Doc, name: &str, def: Value) -> Doc {
    doc.developments
        .push(json!({ "name": name, "definition": def }));
    doc
}

/// The whole loop (delay and its vca) inside a development.
pub fn loop_inside(delay: Value) -> Doc {
    let def = definition(
        vec![
            module("d", "delay", delay),
            module("loop", "vca", json!({ "level": 0.7 })),
        ],
        vec![
            conn("d", "audio", "loop", "audio"),
            conn("loop", "audio", "d", "audio"),
        ],
        ("d", "audio"),
    );
    with_development(gated(30.0), "echo_voice", def)
        .add("voice", "echo_voice", json!({}))
        .wire("src", "audio", "voice", "audio")
        .wire("voice", "audio", "dac", "audio_right")
}

/// The delay in the main graph, the loop's vca inside a development.
pub fn loop_through(delay: Value) -> Doc {
    let def = json!({ "version": "1.0.0",
        "modules": [ module("d", "vca", json!({ "level": 0.7 })) ],
        "connections": [],
        "inputs": [ { "name": "audio", "to": "d", "to_port": "audio" } ],
        "outputs": [ { "name": "audio", "from": "d", "from_port": "audio" } ] });
    with_development(gated(30.0), "gain_stage", def)
        .add("d", "delay", delay)
        .add("stage", "gain_stage", json!({}))
        .wire("src", "audio", "d", "audio")
        .wire("d", "audio", "stage", "audio")
        .wire("stage", "audio", "d", "audio")
        .wire("d", "audio", "dac", "audio_right")
}

// -- rendering ---------------------------------------------------------------

pub fn engine(doc: &Doc) -> RenderEngine {
    let mut engine = RenderEngine::new(SR);
    engine
        .load_invention(doc.invention())
        .expect("the document loads");
    engine
}

/// Renders `frames` and returns the (left, right) outputs.
pub fn render(engine: &mut RenderEngine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).expect("renders");
    let left = out.iter().step_by(2).copied().collect();
    let right = out.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

/// Loads `doc`, renders `frames` at `block`.
pub fn offline(doc: &Doc, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut engine = engine(doc);
    engine.set_block_size(block);
    render(&mut engine, frames)
}

type Callback = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;
struct Capture(Arc<Mutex<Option<Callback>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn start(&mut self, render: Callback) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

/// A running invention whose audio callback the test calls by hand, one
/// `BLOCK` at a time, so an edit's timing is exact.
pub struct Live {
    pub running: RunningInvention,
    slot: Arc<Mutex<Option<Callback>>>,
}

impl Live {
    pub fn start(doc: &Doc) -> Live {
        let (runtime, _) = InventionBuilder::new(SR)
            .build(doc.invention())
            .expect("builds");
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("starts");
        Live { running, slot }
    }
    /// Renders `blocks` blocks and returns (left, right).
    pub fn render(&self, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("started");
        let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for _ in 0..blocks {
            render(&mut l, &mut r);
            left.extend_from_slice(&l);
            right.extend_from_slice(&r);
        }
        (left, right)
    }
    pub fn reload(&mut self, doc: &Doc) {
        self.running
            .reload(doc.invention())
            .unwrap_or_else(|e| panic!("reload applies: {e:?}"));
    }
    pub fn stop(self) {
        self.running.stop();
    }
}

// -- metrics -----------------------------------------------------------------

/// Positions where `x` first reaches `level` after being below it.
pub fn rising_edges(x: &[f32], level: f32) -> Vec<usize> {
    let mut prev = 0.0;
    let mut found = Vec::new();
    for (n, &v) in x.iter().enumerate() {
        if prev < level && v >= level {
            found.push(n);
        }
        prev = v;
    }
    found
}

/// Echo positions relative to the first dry gate in `left[from..to]`: the
/// rising edges of `right` after it, as offsets from that gate.
pub fn echo_offsets(left: &[f32], right: &[f32], from: usize, to: usize) -> Vec<usize> {
    let dry = rising_edges(&left[from..to], EDGE);
    let start = from + *dry.first().expect("a dry gate in the window");
    let echoes = rising_edges(&right[start..to], EDGE);
    echoes
}

/// The first `n` repeats of a delay of `d` samples whose loop is exact.
pub fn exact(d: usize, n: usize) -> Vec<usize> {
    (1..=n).map(|k| k * d).collect()
}

/// The same with the extra sample per pass of per-sample scheduling.
pub fn per_sample(d: usize, n: usize) -> Vec<usize> {
    (1..=n).map(|k| k * d + (k - 1)).collect()
}

pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

pub fn assert_bits(a: &[f32], b: &[f32], label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: lengths differ");
    if let Some(i) = a
        .iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
    {
        panic!(
            "{label}: first difference at sample {i} ({} vs {})",
            a[i], b[i]
        );
    }
}

pub fn assert_close(a: &[f32], b: &[f32], tol: f32, label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: lengths differ");
    if let Some(i) = a.iter().zip(b).position(|(x, y)| !((x - y).abs() <= tol)) {
        panic!(
            "{label}: sample {i} is {} but expected {} (tol {tol})",
            a[i], b[i]
        );
    }
}

// -- the delay atom, driven directly -----------------------------------------

/// The `delay` atom built through the public registry.
pub fn built_delay(config: Value) -> GraphModule {
    ModuleRegistry::default()
        .build("delay", SR, &config)
        .unwrap_or_else(|e| panic!("delay builds with {config}: {e}"))
        .module
}

pub fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            ((s as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
        })
        .collect()
}
