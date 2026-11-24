//! Inventions, an offline renderer, a live capture harness and the device
//! clamp as a reference, plus the player's RMS formula.

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

/// The in-chat player's RMS of one channel (0 left, 1 right) of interleaved
/// device samples: sqrt(sum of squares / frames).
pub fn player_rms(device_samples: &[f32], channel: usize) -> f64 {
    let frames = device_samples.len() / 2;
    let sum: f64 = device_samples
        .iter()
        .skip(channel)
        .step_by(2)
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum();
    (sum / frames as f64).sqrt()
}

/// Largest absolute value of one channel of interleaved samples.
pub fn channel_peak(samples: &[f32], channel: usize) -> f32 {
    samples
        .iter()
        .skip(channel)
        .step_by(2)
        .fold(0.0f32, |m, x| m.max(x.abs()))
}

/// The difference between two levels in decibels.
pub fn db_apart(a: f64, b: f64) -> f64 {
    (20.0 * (a / b).log10()).abs()
}

/// Asserts a reported RMS is within 0.1 dB of the expected one.
pub fn assert_rms(reported: Option<f32>, expected: f64, what: &str) {
    let got = f64::from(reported.unwrap_or_else(|| panic!("{what}: no RMS reported")));
    assert!(
        db_apart(got, expected) <= 0.1,
        "{what}: reported {got}, device samples give {expected} ({} dB apart)",
        db_apart(got, expected)
    );
}
