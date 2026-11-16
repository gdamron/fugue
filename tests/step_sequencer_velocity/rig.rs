//! Offline renders and signal reading shared by the suite.

use fugue::RenderEngine;
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
/// 240 BPM at 48 kHz: a step (one clock beat) is 12,000 samples.
pub const BPM: f64 = 240.0;
pub const STEP: usize = 12_000;
/// Values written through the device clamp: 1.0 reads back a hair below.
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
        for &[l, r] in block.as_chunks::<2>().0 {
            left.push(l);
            right.push(r);
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

/// A step sequencer `seq` with `config`.
pub fn sequencer(config: Value) -> Value {
    json!({ "id": "seq", "type": "step_sequencer", "config": config })
}

/// A sequencer on the clock with two of its signals sent straight to the
/// dac: `left` from `seq.<left>` and `right` from `<right module>.<port>`.
pub fn probe(config: Value, left: &str, right: (&str, &str)) -> String {
    document(
        vec![clock(), sequencer(config), dac()],
        &[
            ["clock", "beat", "seq", "clock"],
            ["seq", left, "dac", "audio_left"],
            [right.0, right.1, "dac", "audio_right"],
        ],
    )
}

/// The samples where `gate` rises past 0.5 (the first sample counts when it
/// starts high).
pub fn rises(gate: &[f32]) -> Vec<usize> {
    (0..gate.len())
        .filter(|&i| gate[i] > 0.5 && (i == 0 || gate[i - 1] <= 0.5))
        .collect()
}

/// The gate's high runs, as `start..end`.
pub fn runs(gate: &[f32]) -> Vec<std::ops::Range<usize>> {
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

/// Asserts the velocity signal changes only on samples where the gate rises.
pub fn assert_changes_only_at_onsets(velocity: &[f32], gate: &[f32], what: &str) {
    let onsets = rises(gate);
    for i in 1..velocity.len() {
        if (velocity[i] - velocity[i - 1]).abs() > TOLERANCE {
            assert!(
                onsets.binary_search(&i).is_ok(),
                "{what}: velocity moved from {} to {} at sample {i}, where no gate rises \
                 (gate {} -> {})",
                velocity[i - 1],
                velocity[i],
                gate[i - 1],
                gate[i],
            );
        }
    }
}

pub fn assert_near(got: f32, want: f32, what: &str) {
    assert!(
        (got - want).abs() <= TOLERANCE,
        "{what}: got {got}, want {want}"
    );
}
