//! One rig over both ways of rendering: the offline engine and a live
//! invention driven by hand through a capturing backend. Both render the
//! left channel of the `dac`, in 64-frame blocks.

use std::sync::{Arc, Mutex};

use fugue::rpc::{ControlTiming, ControlWhen};
use fugue::{
    AudioBackend, ControlValue, ControlWrite, Invention, InventionBuilder, RenderEngine,
    RunningInvention, RuntimeStatus,
};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
pub const BLOCK: usize = 64;
/// A sine of exactly 256 samples a cycle.
pub const TONE_HZ: f64 = 187.5;
/// Frames of steady tone before a pause: 64 + 256 * 20.
pub const BEFORE: usize = 64 + 256 * 20;
/// By this many frames after a pause call the fade (480 frames) has ended,
/// allowing one block to start it and one more to begin the hold.
pub const HOLD_BY: usize = 640;
/// A clock at this tempo counts a beat every 6400 samples (100 blocks).
pub const BPM: f64 = 450.0;
pub const BEAT: usize = 100 * BLOCK;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Offline,
    Live,
}
pub const MODES: [Mode; 2] = [Mode::Offline, Mode::Live];

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
                engine.load_json(&doc.to_string()).expect("loads offline");
                engine.set_block_size(BLOCK);
                Rig(Inner::Offline(engine))
            }
            Mode::Live => {
                let invention: Invention = serde_json::from_value(doc.clone()).expect("parses");
                let (runtime, _) = InventionBuilder::new(RATE)
                    .build(invention)
                    .expect("builds");
                let slot = Arc::new(Mutex::new(None));
                let backend = Capture(slot.clone());
                let running = runtime.start_with_backend(backend).expect("starts");
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
                buffer.chunks(2).map(|frame| frame[0]).collect()
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

    pub fn pause(&self) {
        match &self.0 {
            Inner::Offline(e) => e.pause(),
            Inner::Live { running, .. } => running.pause(),
        }
    }

    pub fn resume(&self) {
        match &self.0 {
            Inner::Offline(e) => e.resume(),
            Inner::Live { running, .. } => running.resume(),
        }
    }

    pub fn is_paused(&self) -> bool {
        match &self.0 {
            Inner::Offline(e) => e.is_paused(),
            Inner::Live { running, .. } => running.is_paused(),
        }
    }

    pub fn status(&self) -> RuntimeStatus {
        match &self.0 {
            Inner::Offline(e) => e.full_snapshot().status,
            Inner::Live { running, .. } => running.full_snapshot().status,
        }
    }

    pub fn set(&self, module: &str, key: &str, value: f32) {
        let value = ControlValue::Number(value);
        match &self.0 {
            Inner::Offline(e) => e.set_control(module, key, value).expect("sets"),
            Inner::Live { running, .. } => running.set_control(module, key, value).expect("sets"),
        }
    }

    pub fn get(&self, module: &str, key: &str) -> f32 {
        let value = match &self.0 {
            Inner::Offline(e) => e.get_control(module, key).expect("gets"),
            Inner::Live { running, .. } => {
                running.announce_applied();
                running.get_control(module, key).expect("gets")
            }
        };
        value.as_number().expect("a number")
    }

    pub fn connect(&mut self, from: &str, to: &str) {
        match &mut self.0 {
            Inner::Offline(e) => e.connect(from, "audio", to, "audio").expect("connects"),
            Inner::Live { running, .. } => running
                .connect(from, "audio", to, "audio")
                .expect("connects"),
        }
    }

    pub fn disconnect(&mut self, from: &str, to: &str) {
        match &mut self.0 {
            Inner::Offline(e) => e
                .disconnect(from, "audio", to, "audio")
                .expect("disconnects"),
            Inner::Live { running, .. } => running
                .disconnect(from, "audio", to, "audio")
                .expect("disconnects"),
        }
    }

    fn live(&mut self) -> &mut RunningInvention {
        match &mut self.0 {
            Inner::Live { running, .. } => running,
            Inner::Offline(_) => panic!("live only"),
        }
    }

    /// Reloads the running invention with `doc`.
    pub fn reload(&mut self, doc: &Value) {
        let invention: Invention = serde_json::from_value(doc.clone()).expect("parses");
        self.live().reload(invention).expect("reloads");
    }

    /// Writes `value` to `module`'s `key` at `when`.
    pub fn timed(&mut self, module: &str, key: &str, value: f32, when: ControlWhen) {
        let timing = ControlTiming {
            when: Some(when),
            ttl_ms: None,
        };
        let number = ControlValue::Number(value);
        let write = ControlWrite {
            timing,
            ..ControlWrite::performed(module, key, number)
        };
        let acks = self.live().write_controls(&[write]);
        assert!(acks.error.is_none(), "{:?}", acks.error);
    }

    /// Unloads the piece, as the daemon does.
    pub fn unload(self) {
        match self.0 {
            Inner::Live { running, .. } => running.stop(),
            Inner::Offline(_) => panic!("live only"),
        }
    }
}

// -- documents ---------------------------------------------------------------

/// A sine at `hz` into the dac, and a clock at `BPM`; the sine is wired in
/// unless `wired` is false.
pub fn document(hz: f64, wired: bool) -> Value {
    let connections = if wired {
        json!([{ "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }])
    } else {
        json!([])
    };
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": hz } },
            { "id": "clock", "type": "clock", "config": { "bpm": BPM } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": connections
    })
}

// -- measurements --------------------------------------------------------------

/// The largest |y[i] - y[i-1]| for i in `from..y.len()`.
pub fn max_step(y: &[f32], from: usize) -> f32 {
    (from.max(1)..y.len())
        .map(|i| (y[i] - y[i - 1]).abs())
        .fold(0.0, f32::max)
}

pub fn max_abs(y: &[f32]) -> f32 {
    y.iter().fold(0.0, |m, v| m.max(v.abs()))
}

pub fn all_zero(y: &[f32]) -> bool {
    y.iter().all(|v| *v == 0.0)
}

/// How many times the signal changes sign.
pub fn crossings(y: &[f32]) -> usize {
    y.windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count()
}
