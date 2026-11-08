//! An offline render writes what the device would play, while the peak meter
//! keeps reading the mix before the clamp.

use super::RenderEngine;
use crate::modules::dac::{device_sample, MAX_BELOW_ONE};

/// A sine at +12 dB into a dac without soft clip: peaks near 4.
const LOUD: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 441.0 } },
        { "id": "gain", "type": "gain", "config": { "decibels": 12.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "gain", "to_port": "audio" },
        { "from": "gain", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn an_over_level_render_is_clamped_and_the_meter_reads_the_mix() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(LOUD).unwrap();
    let mut output = vec![0.0f32; 2 * 4_800];
    engine.render_interleaved(&mut output).unwrap();

    assert!(output.iter().all(|x| (-1.0..=MAX_BELOW_ONE).contains(x)));
    assert_eq!(
        output.iter().copied().fold(f32::MIN, f32::max),
        MAX_BELOW_ONE
    );
    assert_eq!(output.iter().copied().fold(f32::MAX, f32::min), -1.0);

    let graph = engine.graph.as_ref().unwrap().lock().unwrap();
    let levels = graph.master.levels.drain(48_000);
    assert!(
        levels.left_peak > 3.9,
        "pre-clamp peak {}",
        levels.left_peak
    );
    assert!(
        levels.right_peak > 3.9,
        "pre-clamp peak {}",
        levels.right_peak
    );
}

#[test]
fn the_device_clamp_leaves_in_range_samples_untouched() {
    for sample in [0.0, -0.0, 0.5, -1.0, MAX_BELOW_ONE, f32::MIN_POSITIVE] {
        assert_eq!(device_sample(sample).to_bits(), sample.to_bits());
    }
}
