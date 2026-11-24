//! A clocked invention and a renderer.

use fugue::RenderEngine;
use serde_json::json;

pub const RATE: u32 = 48_000;
pub const CLOCK: &str = "clock";
pub const BPM: f64 = 120.0;

/// A clock beside a sine through a gain into a dac. At +12 dB the mix goes
/// over full scale, so the levels have overs to compare; at -9 dB it does not.
pub fn piece(decibels: f64) -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": CLOCK, "type": "clock", "config": { "bpm": BPM } },
            { "id": "osc", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": 311.0 } },
            { "id": "gain", "type": "gain", "config": { "decibels": decibels } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "gain", "to_port": "audio" },
            { "from": "gain", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    })
    .to_string()
}

/// A seeded engine with `piece(decibels)` loaded.
pub fn engine(decibels: f64) -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.set_seed(Some(1));
    engine.load_json(&piece(decibels)).unwrap();
    engine
}

/// Renders `frames` frames in buffers of `chunk` frames.
pub fn render(engine: &mut RenderEngine, frames: usize, chunk: usize) {
    let mut done = 0;
    while done < frames {
        let n = chunk.min(frames - done);
        let mut out = vec![0.0f32; n * 2];
        assert_eq!(engine.render_interleaved(&mut out).unwrap(), n);
        done += n;
    }
}

/// The clock's `elapsed` control as `get_control` reports it.
pub fn elapsed(engine: &RenderEngine) -> f64 {
    f64::from(
        engine
            .get_control(CLOCK, "elapsed")
            .unwrap()
            .as_number()
            .unwrap(),
    )
}

/// Whether two beat counts agree within f32 precision.
pub fn same_beats(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}
