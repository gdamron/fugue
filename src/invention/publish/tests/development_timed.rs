//! A timed write to a development's control lands on its sample, as a write
//! to a native module does, at every graph block size.
//!
//! Each case renders a document untouched and again with timed writes; the
//! first sample where the two differ is where the first write landed.

use super::*;
use crate::control_request::{RtValue, When};

/// A voice's oscillator waveform changing from sine to square snaps: the
/// output differs from the untouched render at the very sample it lands.
const SQUARE: RtValue = RtValue::U32(1);

const BLOCKS: [usize; 4] = [64, 128, 512, 1024];

fn osc(id: &str, freq: f64) -> serde_json::Value {
    serde_json::json!({ "id": id, "type": "oscillator",
        "config": { "waveform": "sine", "frequency": freq, "frequency_mod_depth": 20.0 } })
}

fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> serde_json::Value {
    serde_json::json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// A main graph with a development `voice` into the dac. `controls` lists
/// the development's exposed controls as `(name, inner module, control)`.
fn development(
    modules: Vec<serde_json::Value>,
    connections: Vec<serde_json::Value>,
    out: &str,
    controls: &[(&str, &str, &str)],
) -> String {
    let controls: Vec<_> = controls
        .iter()
        .map(|(name, module, control)| {
            serde_json::json!({ "name": name, "module": module, "control": control })
        })
        .collect();
    serde_json::json!({
        "version": "1.0.0",
        "developments": [{ "name": "pair", "definition": {
            "version": "1.0.0",
            "modules": modules,
            "connections": connections,
            "outputs": [{ "name": "audio", "from": out, "from_port": "audio" }],
            "controls": controls,
        } }],
        "modules": [
            { "id": "voice", "type": "pair", "config": {} },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [conn("voice", "audio", "dac", "audio")],
    })
    .to_string()
}

/// The same wiring placed flat in the main graph.
fn flat(
    mut modules: Vec<serde_json::Value>,
    connections: Vec<serde_json::Value>,
    out: &str,
) -> String {
    modules
        .push(serde_json::json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }));
    let mut connections = connections;
    connections.push(conn(out, "audio", "dac", "audio"));
    serde_json::json!({ "version": "1.0.0", "modules": modules, "connections": connections })
        .to_string()
}

/// One oscillator exposing its waveform as `shape`, as development and as
/// the native oscillator `voice`.
fn single() -> (String, String) {
    let dev = development(
        vec![osc("inner", 137.0)],
        vec![],
        "inner",
        &[("shape", "inner", "waveform")],
    );
    (dev, flat(vec![osc("voice", 137.0)], vec![], "voice"))
}

/// Two oscillators frequency-modulating each other (a feedback loop) into a
/// vca; `shape` reaches the first. Flat, the first is called `voice`.
fn looped() -> (String, String) {
    let wiring = |first: &str| {
        let modules = vec![
            osc(first, 137.0),
            osc("second", 211.0),
            serde_json::json!({ "id": "out", "type": "vca", "config": { "level": 0.5 } }),
        ];
        let connections = vec![
            conn(first, "audio", "second", "frequency_mod"),
            conn("second", "audio", first, "frequency_mod"),
            conn(first, "audio", "out", "audio"),
        ];
        (modules, connections)
    };
    let (modules, connections) = wiring("inner");
    let dev = development(
        modules,
        connections,
        "out",
        &[("shape", "inner", "waveform")],
    );
    let (modules, connections) = wiring("voice");
    (dev, flat(modules, connections, "out"))
}

/// Renders `doc` in graph blocks of `block` for `frames`, with each
/// `(sample, module, key, value)` submitted before the first block. Returns
/// the left channel.
fn render(
    doc: &str,
    block: usize,
    frames: usize,
    writes: &[(u64, &str, &str, RtValue)],
) -> Vec<f32> {
    let mut rig = Rig::new(doc);
    rig.graph.set_block_size(block);
    let start = rig.graph.transport.rendered();
    for (sample, module, key, value) in writes {
        rig.live
            .submit_logged((module, key), *value, When::AtSample(start + sample), None);
    }
    let (mut left, mut right) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        rig.graph.process_block(&mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out
}

fn first_difference(base: &[f32], changed: &[f32]) -> Option<usize> {
    base.iter()
        .zip(changed)
        .position(|(a, b)| a.to_bits() != b.to_bits())
}

/// The sample a single write of `SQUARE` to `module`'s `key` lands on.
fn landing(doc: &str, block: usize, module: &str, key: &str, sample: u64) -> Option<usize> {
    let frames = block * 4;
    let base = render(doc, block, frames, &[]);
    let changed = render(doc, block, frames, &[(sample, module, key, SQUARE)]);
    first_difference(&base, &changed)
}

/// Samples to write at in a block of `block`: early, middle, late, and in a
/// later block.
fn samples(block: usize) -> Vec<u64> {
    let b = block as u64;
    vec![1, b / 2 + 5, b - 1, b + 1, 2 * b + b / 3]
}

#[test]
fn a_native_write_lands_on_its_sample() {
    let (_, native) = single();
    for block in BLOCKS {
        for n in samples(block) {
            assert_eq!(
                landing(&native, block, "voice", "waveform", n),
                Some(n as usize),
                "block {block}"
            );
        }
    }
}

#[test]
fn a_development_write_lands_on_its_sample() {
    let (dev, _) = single();
    for block in BLOCKS {
        for n in samples(block) {
            assert_eq!(
                landing(&dev, block, "voice", "shape", n),
                Some(n as usize),
                "block {block}"
            );
        }
    }
}

#[test]
fn a_write_into_a_development_loop_lands_on_its_sample() {
    let (dev, _) = looped();
    for block in BLOCKS {
        for n in samples(block) {
            assert_eq!(
                landing(&dev, block, "voice", "shape", n),
                Some(n as usize),
                "block {block}"
            );
        }
    }
}
