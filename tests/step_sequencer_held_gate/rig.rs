//! Offline renders of a clocked step sequencer, and gate reading.

use std::ops::Range;

use fugue::RenderEngine;
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
/// 240 BPM at 48 kHz: a step (one clock beat) is 12,000 samples.
pub const BPM: f64 = 240.0;
pub const STEP: usize = 12_000;
pub const TOLERANCE: f32 = 1e-5;

/// Renders `json` offline for `frames` frames, as (left, right).
pub fn render(json: &str, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut engine = RenderEngine::new(RATE);
    engine.set_seed(Some(1));
    engine
        .load_json(json)
        .unwrap_or_else(|err| panic!("the invention loads: {err}"));
    let mut block = vec![0.0f32; 1024];
    let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    while left.len() < frames {
        engine.render_interleaved(&mut block).unwrap();
        for pair in block.chunks_exact(2) {
            left.push(pair[0]);
            right.push(pair[1]);
        }
    }
    left.truncate(frames);
    right.truncate(frames);
    (left, right)
}

/// A document of `modules` and `connections` (each `[from, port, to, port]`).
pub fn document(modules: Vec<Value>, connections: &[[&str; 4]]) -> String {
    let connections: Vec<Value> = connections
        .iter()
        .map(|[from, from_port, to, to_port]| {
            json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
        })
        .collect();
    json!({ "version": "1.0.0", "modules": modules, "connections": connections }).to_string()
}

pub fn clock() -> Value {
    json!({ "id": "clock", "type": "clock", "config": { "bpm": BPM } })
}

pub fn dac() -> Value {
    json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } })
}

/// A step sequencer `seq` playing `pattern` (root 60, default gate 0.5),
/// with `extra` config merged in.
pub fn sequencer(pattern: Value, extra: Value) -> Value {
    let mut config = json!({
        "root_note": 60,
        "step_count": pattern.as_array().unwrap().len(),
        "gate_length": 0.5,
        "pattern": pattern,
    });
    for (key, value) in extra.as_object().unwrap() {
        config[key] = value.clone();
    }
    json!({ "id": "seq", "type": "step_sequencer", "config": config })
}

/// Renders a clocked sequencer for `steps` clock steps (plus a little),
/// returning `(seq.<port>, clock edges)`: the sample each clock beat rises
/// on, which is the sample the sequencer starts that step.
pub fn play(pattern: Value, extra: Value, port: &str, steps: usize) -> (Vec<f32>, Vec<usize>) {
    let json = document(
        vec![clock(), sequencer(pattern, extra), dac()],
        &[
            ["clock", "beat", "seq", "clock"],
            ["seq", port, "dac", "audio_left"],
            ["clock", "beat", "dac", "audio_right"],
        ],
    );
    let (signal, beat) = render(&json, steps * STEP + STEP / 2);
    (signal, rises(&beat))
}

/// The samples where `gate` rises past 0.5 (the first sample counts when it
/// starts high).
pub fn rises(gate: &[f32]) -> Vec<usize> {
    (0..gate.len())
        .filter(|&i| gate[i] > 0.5 && (i == 0 || gate[i - 1] <= 0.5))
        .collect()
}

/// The gate's high runs, as `start..end` (`end` is the first low sample).
pub fn runs(gate: &[f32]) -> Vec<Range<usize>> {
    rises(gate)
        .into_iter()
        .map(|start| {
            let end = (start..gate.len())
                .find(|&i| gate[i] <= 0.5)
                .unwrap_or(gate.len());
            start..end
        })
        .collect()
}

/// Asserts the gate is high on every sample of `range`.
pub fn assert_high(gate: &[f32], range: Range<usize>, what: &str) {
    if let Some(i) = range.clone().find(|&i| gate[i] <= 0.5) {
        panic!(
            "{what}: the gate is low at sample {i}, inside {range:?} where it should hold \
             (runs: {:?})",
            runs(gate)
        );
    }
}

/// Asserts the gate is low on every sample of `range`.
pub fn assert_low(gate: &[f32], range: Range<usize>, what: &str) {
    if let Some(i) = range.clone().find(|&i| gate[i] > 0.5) {
        panic!(
            "{what}: the gate is high at sample {i}, inside {range:?} where it should be low \
             (runs: {:?})",
            runs(gate)
        );
    }
}

/// Asserts `signal` reads `want` on every sample of `range`.
pub fn assert_value(signal: &[f32], range: Range<usize>, want: f32, what: &str) {
    let off = signal[range.clone()]
        .iter()
        .position(|&value| (value - want).abs() > TOLERANCE);
    if let Some(offset) = off {
        let i = range.start + offset;
        panic!("{what}: {} at sample {i}, want {want}", signal[i]);
    }
}
