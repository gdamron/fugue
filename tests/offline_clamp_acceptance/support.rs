//! Inventions, an offline renderer, a live capture harness and the device
//! clamp as a reference.

use std::sync::{Arc, Mutex};

use fugue::{AudioBackend, Invention, InventionBuilder, RenderEngine, RunningInvention};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;

/// The largest `f32` below 1.0: where the live device clamp stops.
pub const MAX_BELOW_ONE: f32 = 1.0 - f32::EPSILON / 2.0;

/// What the live device writes for a sample the graph produced.
pub fn device(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, MAX_BELOW_ONE)
    } else {
        0.0
    }
}

fn wire(from: &str, to: &str) -> Value {
    json!({ "from": from, "from_port": "audio", "to": to, "to_port": "audio" })
}

fn osc(waveform: &str, frequency: f64) -> Value {
    json!({ "id": "osc", "type": "oscillator",
            "config": { "waveform": waveform, "frequency": frequency } })
}

fn dac(soft_clip: bool) -> Value {
    json!({ "id": "dac", "type": "dac", "config": { "soft_clip": soft_clip } })
}

fn document(modules: Vec<Value>, connections: Vec<Value>) -> String {
    json!({ "version": "1.0.0", "modules": modules, "connections": connections }).to_string()
}

/// An oscillator through one gain stage into a dac without soft clip.
pub fn tone(waveform: &str, frequency: f64, decibels: f64) -> String {
    document(
        vec![
            osc(waveform, frequency),
            json!({ "id": "gain", "type": "gain", "config": { "decibels": decibels } }),
            dac(false),
        ],
        vec![wire("osc", "gain"), wire("gain", "dac")],
    )
}

/// A sine through the dac's default soft clip, no gain stage.
pub fn soft_clipped_sine() -> String {
    document(
        vec![
            osc("sine", 440.0),
            json!({ "id": "dac", "type": "dac", "config": {} }),
        ],
        vec![wire("osc", "dac")],
    )
}

/// A sine into 33 gain stages at +24 dB: the last overflows to infinity over
/// the loud part of each cycle and is silenced by the dac, while samples
/// near each zero crossing stay finite but enormous.
pub fn overflowing_chain() -> String {
    let mut modules = vec![osc("sine", 997.0), dac(false)];
    let mut connections = vec![wire("osc", "g0")];
    for i in 0..33 {
        modules
            .push(json!({ "id": format!("g{i}"), "type": "gain", "config": { "decibels": 24.0 } }));
        if i > 0 {
            connections.push(wire(&format!("g{}", i - 1), &format!("g{i}")));
        }
    }
    connections.push(wire("g32", "dac"));
    document(modules, connections)
}

/// `frames` frames rendered offline, seeded, in buffers of `chunk` frames
/// (the last one shorter), as interleaved stereo.
pub fn offline(json: &str, frames: usize, chunk: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.set_seed(Some(1));
    engine.load_json(json).unwrap();
    let mut all = Vec::with_capacity(frames * 2);
    let mut done = 0;
    while done < frames {
        let n = chunk.min(frames - done);
        let mut out = vec![f32::NAN; n * 2];
        assert_eq!(engine.render_interleaved(&mut out).unwrap(), n);
        all.extend(out);
        done += n;
    }
    all
}

/// The samples' bits folded into one FNV-1a hash.
pub fn hash(samples: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
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

/// An invention playing live through a capturing backend, driven like a
/// device callback. What it captures is what the graph hands the device,
/// before the device writes it out.
pub struct Live {
    pub running: RunningInvention,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Live {
    pub fn start(json: &str) -> Live {
        let doc = Invention::from_json(json).unwrap();
        let (runtime, _) = InventionBuilder::new(RATE).build(doc).unwrap();
        let slot = Arc::new(Mutex::new(None));
        let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
        Live { running, slot }
    }

    /// `frames` frames (a multiple of 64) in callbacks of 64, interleaved.
    pub fn capture(&self, frames: usize) -> Vec<f32> {
        assert_eq!(frames % 64, 0);
        let mut all = Vec::with_capacity(frames * 2);
        for _ in 0..frames / 64 {
            let (mut l, mut r) = ([f32::NAN; 64], [f32::NAN; 64]);
            (self.slot.lock().unwrap().as_mut().expect("backend started"))(&mut l, &mut r);
            for i in 0..64 {
                all.push(l[i]);
                all.push(r[i]);
            }
        }
        all
    }
}
