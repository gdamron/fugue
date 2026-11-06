//! Offline render. `RenderEngine` builds from the built-in registry, so the
//! signals come from chains of built-in stages on a sine:
//! - 33 `gain` stages at +24 dB overflow to +/-inf at the last stage over
//!   the loud part of each cycle (a gain turns a non-finite input into zero,
//!   so only the last stage's overflow reaches the dac);
//! - 40 `vca` stages each multiplying by the sine (`sine^41`, positive half
//!   only) pass through the subnormal range near each zero crossing.

use super::*;
use fugue::RenderEngine;

const RATE: u32 = 48_000;

fn osc() -> Value {
    json!({ "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 997.0 } })
}

fn wire(from: &str, to: &str, port: &str) -> Value {
    json!({ "from": from, "from_port": "audio", "to": to, "to_port": port })
}

fn document(modules: Vec<Value>, connections: Vec<Value>) -> String {
    json!({ "version": "1.0.0", "modules": modules, "connections": connections }).to_string()
}

/// osc -> 33 gains at +24 dB in series -> dac.
fn overflow_chain() -> String {
    let mut modules = vec![osc(), json!({ "id": "dac", "type": "dac", "config": {} })];
    let mut connections = vec![wire("osc", "g0", "audio")];
    for i in 0..33 {
        modules
            .push(json!({ "id": format!("g{i}"), "type": "gain", "config": { "decibels": 24.0 } }));
        if i > 0 {
            connections.push(wire(&format!("g{}", i - 1), &format!("g{i}"), "audio"));
        }
    }
    connections.push(wire("g32", "dac", "audio"));
    document(modules, connections)
}

/// osc -> 40 vcas in series, each controlled by the osc itself -> dac.
fn decay_chain() -> String {
    let mut modules = vec![osc(), json!({ "id": "dac", "type": "dac", "config": {} })];
    let mut connections = vec![wire("osc", "v0", "audio")];
    for i in 0..40 {
        modules.push(json!({ "id": format!("v{i}"), "type": "vca", "config": {} }));
        connections.push(wire("osc", &format!("v{i}"), "level"));
        if i > 0 {
            connections.push(wire(&format!("v{}", i - 1), &format!("v{i}"), "audio"));
        }
    }
    connections.push(wire("v39", "dac", "audio"));
    document(modules, connections)
}

/// Renders `chunks` consecutive buffers of `frames` frames, checking the
/// calling thread is left as found after each.
fn render(json: &str, chunks: usize, frames: usize) -> Vec<Vec<f32>> {
    assert!(!fugue::flush_to_zero_enabled(), "off before a render");
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(json).unwrap();
    (0..chunks)
        .map(|_| {
            let mut out = vec![f32::MAX; frames * 2];
            engine.render_interleaved(&mut out).unwrap();
            assert!(!fugue::flush_to_zero_enabled(), "restored after a render");
            out
        })
        .collect()
}

#[test]
fn overflowing_signal_is_written_as_silence_not_nan() {
    // |sine| > 0.085 (about 95% of the samples) overflows at the last gain
    // stage and must reach the output as 0.0. The few samples near a zero
    // crossing are huge but finite; what the dac does with those is not
    // asserted, only that nothing infinite is written.
    let out = &render(&overflow_chain(), 2, 4_800)[1];
    assert!(
        out.iter().all(|x| !x.is_infinite()),
        "no infinity is written"
    );
    for side in 0..2 {
        let silent = out
            .iter()
            .skip(side)
            .step_by(2)
            .filter(|&&x| x == 0.0)
            .count();
        assert!(
            silent >= 4_800 * 80 / 100,
            "overflowing samples are silence ({silent} of 4800)"
        );
    }
}

#[test]
fn render_flushes_subnormals_on_every_render() {
    // The chain is audible at its peaks and decays through the subnormal
    // range at the crossings; with flush-to-zero none survives.
    for out in render(&decay_chain(), 2, 4_800) {
        assert!(out.iter().any(|x| x.abs() > 0.5), "the chain is audible");
        let subnormals = out
            .iter()
            .filter(|x| **x != 0.0 && x.abs() < f32::MIN_POSITIVE)
            .count();
        if FTZ_TARGET {
            assert_eq!(subnormals, 0, "no subnormal in the output");
        }
    }
}
