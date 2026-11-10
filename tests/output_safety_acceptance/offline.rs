//! Offline render. `RenderEngine` builds from the built-in registry, so the
//! signal comes from a chain of built-in stages on a sine: 40 `vca` stages
//! each multiplying by the sine (`sine^41`, positive half only) pass through
//! the subnormal range near each zero crossing.

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
