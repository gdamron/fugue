//! One rig over both ways of rendering: the offline engine and a live
//! invention driven by hand through a capturing backend. Both render the
//! left channel of the `dac`, in 64-frame blocks.

use std::sync::{Arc, Mutex};

use fugue::{AudioBackend, Invention, InventionBuilder, RenderEngine, RunningInvention};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
pub const BLOCK: usize = 64;
/// Samples in 1 ms.
pub const MS: usize = RATE as usize / 1000;

/// A held sine of exactly 256 samples a cycle (187.5 Hz), whose peaks fall
/// on samples 64 + 256k, so an edit made before such a sample lands on a
/// block boundary right at a peak.
pub const TONE_HZ: f64 = 187.5;
/// Frames rendered before an edit: 64 + 256 * 20, a block boundary at a peak.
pub const BEFORE: usize = 64 + 256 * 20;
/// Frames rendered after an edit when watching a parameter settle (50.7 ms).
pub const AFTER: usize = 38 * BLOCK;
/// Frames rendered after an edit when watching a source fall silent (100 ms).
pub const AFTER_LONG: usize = 75 * BLOCK;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Offline,
    Live,
}

/// A connection, as (from module, from port, to module, to port).
#[derive(Clone, Copy, Debug)]
pub struct Edge(
    pub &'static str,
    pub &'static str,
    pub &'static str,
    pub &'static str,
);

impl Edge {
    pub fn json(&self) -> Value {
        json!({ "from": self.0, "from_port": self.1, "to": self.2, "to_port": self.3 })
    }
}

/// A change made to the graph while it plays.
#[derive(Clone, Debug)]
pub enum Edit {
    Disconnect(Edge),
    Connect(Edge),
    Remove(&'static str),
    /// Reload the running invention with this document (live only).
    Reload(Value),
}

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

enum Inner {
    Offline(RenderEngine),
    Live {
        running: RunningInvention,
        slot: Arc<Mutex<Option<Render>>>,
    },
}

pub struct Rig(Inner);

impl Rig {
    pub fn start(mode: Mode, doc: &Value) -> Rig {
        match mode {
            Mode::Offline => {
                let mut engine = RenderEngine::new(RATE);
                engine
                    .load_json(&doc.to_string())
                    .unwrap_or_else(|e| panic!("document loads offline: {e}\n{doc:#}"));
                engine.set_block_size(BLOCK);
                Rig(Inner::Offline(engine))
            }
            Mode::Live => {
                let invention: Invention = serde_json::from_value(doc.clone())
                    .unwrap_or_else(|e| panic!("document parses: {e}\n{doc:#}"));
                let (runtime, _) = InventionBuilder::new(RATE)
                    .build(invention)
                    .unwrap_or_else(|e| panic!("document builds live: {e}\n{doc:#}"));
                let slot = Arc::new(Mutex::new(None));
                let running = runtime
                    .start_with_backend(Capture(slot.clone()))
                    .expect("a capturing backend starts");
                Rig(Inner::Live { running, slot })
            }
        }
    }

    /// The left channel of `frames` frames (a multiple of 64).
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        assert_eq!(frames % BLOCK, 0, "render whole blocks");
        match &mut self.0 {
            Inner::Offline(engine) => {
                let mut buffer = vec![0.0f32; frames * 2];
                engine.render_interleaved(&mut buffer).expect("renders");
                buffer.chunks_exact(2).map(|frame| frame[0]).collect()
            }
            Inner::Live { slot, .. } => {
                let mut out = Vec::with_capacity(frames);
                let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
                let mut slot = slot.lock().unwrap();
                let render = slot.as_mut().expect("backend started");
                for _ in 0..frames / BLOCK {
                    render(&mut left, &mut right);
                    out.extend_from_slice(&left);
                }
                out
            }
        }
    }

    /// Makes `edit`, to land at the next block boundary.
    pub fn apply(&mut self, edit: &Edit) {
        match (&mut self.0, edit) {
            (Inner::Offline(e), Edit::Disconnect(Edge(a, b, c, d))) => {
                e.disconnect(a, b, c, d).expect("disconnects offline")
            }
            (Inner::Offline(e), Edit::Connect(Edge(a, b, c, d))) => {
                e.connect(a, b, c, d).expect("connects offline")
            }
            (Inner::Offline(e), Edit::Remove(id)) => e.remove_module(id).expect("removes offline"),
            (Inner::Live { running, .. }, Edit::Disconnect(Edge(a, b, c, d))) => {
                running.disconnect(a, b, c, d).expect("disconnects live")
            }
            (Inner::Live { running, .. }, Edit::Connect(Edge(a, b, c, d))) => {
                running.connect(a, b, c, d).expect("connects live")
            }
            (Inner::Live { running, .. }, Edit::Remove(id)) => {
                running.remove_module(*id).expect("removes live")
            }
            (Inner::Live { running, .. }, Edit::Reload(doc)) => {
                let invention: Invention = serde_json::from_value(doc.clone()).expect("parses");
                running.reload(invention).expect("reloads");
            }
            (Inner::Offline(_), Edit::Reload(_)) => panic!("no offline reload"),
        }
    }
}

// -- documents ---------------------------------------------------------------

pub fn module(id: &str, kind: &str, config: Value) -> Value {
    json!({ "id": id, "type": kind, "config": config })
}

pub fn dac() -> Value {
    module("dac", "dac", json!({ "soft_clip": false }))
}

pub fn wire(from: &str, to: &str, port: &str) -> Value {
    json!({ "from": from, "from_port": "audio", "to": to, "to_port": port })
}

pub fn document(modules: Vec<Value>, connections: Vec<Value>) -> Value {
    json!({ "version": "1.0.0", "modules": modules, "connections": connections })
}

/// The tone held through every test: a sine of exactly 256 samples a cycle.
pub fn tone(id: &str, extra: Value) -> Value {
    let mut config = json!({ "waveform": "sine", "frequency": TONE_HZ });
    for (key, value) in extra.as_object().into_iter().flatten() {
        config[key] = value.clone();
    }
    module(id, "oscillator", config)
}

/// A square that stays at +1.0 for the first seconds: a constant source.
pub fn constant(id: &str) -> Value {
    module(
        id,
        "oscillator",
        json!({ "waveform": "square", "frequency": 0.001 }),
    )
}

/// Renders `doc` offline for `frames` frames.
pub fn render_offline(doc: &Value, frames: usize) -> Vec<f32> {
    Rig::start(Mode::Offline, doc).render(frames)
}

// -- measurements --------------------------------------------------------------

/// The largest |y[i] - y[i-1]| for i in `from..to`.
pub fn max_step(y: &[f32], from: usize, to: usize) -> f32 {
    (from.max(1)..to.min(y.len()))
        .map(|i| (y[i] - y[i - 1]).abs())
        .fold(0.0, f32::max)
}

pub fn max_abs(y: &[f32]) -> f32 {
    y.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// The largest |a[i] - b[i]| for i in `from..to`.
pub fn max_difference(a: &[f32], b: &[f32], from: usize, to: usize) -> f32 {
    (from..to.min(a.len()).min(b.len()))
        .map(|i| (a[i] - b[i]).abs())
        .fold(0.0, f32::max)
}

/// Whether the 20 ms after `edit_at` step no more than `limit` times the
/// 20 ms before it; the error says what was measured.
pub fn steps_within(y: &[f32], edit_at: usize, limit: f32) -> Result<(), String> {
    let window = 20 * MS;
    let steady = max_step(y, edit_at - window, edit_at);
    if steady < 0.01 {
        return Err(format!(
            "the steady step ({steady}) is too small to measure"
        ));
    }
    let after = max_step(y, edit_at, edit_at + window);
    if after > limit * steady {
        return Err(format!(
            "largest step after the edit {after:.5} is {:.1}x the steady step {steady:.5} \
             (limit {limit}x)",
            after / steady
        ));
    }
    Ok(())
}

/// Collects the failures of several cases so one run reports them all.
#[derive(Default)]
pub struct Cases(Vec<String>);

impl Cases {
    pub fn check(&mut self, label: impl std::fmt::Display, result: Result<(), String>) {
        if let Err(why) = result {
            self.0.push(format!("{label}: {why}"));
        }
    }

    pub fn finish(self) {
        assert!(self.0.is_empty(), "\n{}", self.0.join("\n"));
    }
}
