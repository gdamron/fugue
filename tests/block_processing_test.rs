//! Block-size invariance tests for the block-based signal graph.
//!
//! Processing the same invention with different block sizes must produce
//! identical audio: acyclic modules run a whole block at a time, while feedback
//! cycles fall back to sample-by-sample so a back-edge always observes a true
//! one-sample delay regardless of where block boundaries land.

use fugue::RenderEngine;

/// Renders `frames` stereo frames of `json` at the given `block_size`.
fn render_at(json: &str, frames: usize, block_size: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(json).unwrap();
    engine.set_block_size(block_size);
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

/// Asserts two renders are bit-for-bit identical.
fn assert_identical(a: &[f32], b: &[f32], label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: length mismatch");
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            x, y,
            "{label}: sample {i} differs ({x} vs {y}) — block size changed the output"
        );
    }
}

/// An acyclic voice: clock → adsr → vca.cv, lfo → osc.fm, osc → filter → vca → dac.
const ACYCLIC_INVENTION: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 128.0, "gate_duration": 0.4 } },
        { "id": "lfo", "type": "lfo", "config": { "frequency": 5.0, "waveform": "triangle" } },
        { "id": "osc", "type": "oscillator", "config": { "oscillator_type": "sawtooth", "frequency": 220.0, "fm_amount": 40.0 } },
        { "id": "adsr", "type": "adsr", "config": { "attack": 0.005, "decay": 0.1, "sustain": 0.6, "release": 0.2 } },
        { "id": "filter", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": 1200.0, "resonance": 0.4 } },
        { "id": "vca", "type": "vca", "config": { "cv": 0.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": true } }
    ],
    "connections": [
        { "from": "lfo", "from_port": "out", "to": "osc", "to_port": "fm" },
        { "from": "osc", "from_port": "audio", "to": "filter", "to_port": "audio" },
        { "from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate" },
        { "from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv" },
        { "from": "filter", "from_port": "audio", "to": "vca", "to_port": "audio" },
        { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// A feedback patch: osc1 ↔ osc2 cross-FM (a 2-node SCC), osc1 → dac.
const FEEDBACK_INVENTION: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 220.0, "fm_amount": 80.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 110.0, "fm_amount": 80.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "osc2", "to_port": "fm" },
        { "from": "osc2", "from_port": "audio", "to": "osc1", "to_port": "fm" },
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// A multi-source mix into one DAC port must sum (not last-source-wins).
const MIX_INVENTION: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 330.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 440.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn acyclic_render_is_block_size_invariant() {
    let frames = 4096;
    let baseline = render_at(ACYCLIC_INVENTION, frames, 1);
    for &block in &[2usize, 31, 64, 128, 512, 1024] {
        let other = render_at(ACYCLIC_INVENTION, frames, block);
        assert_identical(&baseline, &other, &format!("acyclic block={block}"));
    }
    // Sanity: the render actually produced signal.
    assert!(baseline.iter().any(|s| s.abs() > 0.0));
}

#[test]
fn feedback_render_is_block_size_invariant() {
    let frames = 4096;
    let baseline = render_at(FEEDBACK_INVENTION, frames, 1);
    for &block in &[2usize, 17, 64, 128, 512] {
        let other = render_at(FEEDBACK_INVENTION, frames, block);
        assert_identical(&baseline, &other, &format!("feedback block={block}"));
    }
    assert!(baseline.iter().any(|s| s.abs() > 0.0));
}

#[test]
fn multiple_sources_into_one_port_sum() {
    let frames = 256;
    // Two oscillators summed into the DAC's `audio` port. Compared against each
    // rendered alone, the combined render should equal the sum per frame.
    let combined = render_at(MIX_INVENTION, frames, 64);

    let only1 = render_at(
        r#"{ "version": "1.0.0",
             "modules": [
                { "id": "osc1", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 330.0 } },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } } ],
             "connections": [ { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" } ] }"#,
        frames,
        64,
    );
    let only2 = render_at(
        r#"{ "version": "1.0.0",
             "modules": [
                { "id": "osc2", "type": "oscillator", "config": { "oscillator_type": "sine", "frequency": 440.0 } },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } } ],
             "connections": [ { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" } ] }"#,
        frames,
        64,
    );

    for i in 0..combined.len() {
        let expected = only1[i] + only2[i];
        assert!(
            (combined[i] - expected).abs() < 1e-6,
            "sample {i}: sources did not sum ({} vs {})",
            combined[i],
            expected
        );
    }
}
