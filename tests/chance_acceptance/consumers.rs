//! Consumers: a chance gate triggers a `sample_kit` and a chance frequency
//! drives an `oscillator`, both through the graph.

use crate::quantize::{in_scale, midi_of};
use crate::rig::*;
use fugue::RenderEngine;
use std::path::PathBuf;

fn temp_wav(level: f32, frames: usize) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("fugue-chance-e2e-{nanos}-{level}.wav"));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..frames {
        writer
            .write_sample((level * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
    path
}

fn kit_invention(kick: &std::path::Path, chance: serde_json::Value, gate: &str) -> String {
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "chance", "type": "chance", "config": chance },
            { "id": "kit", "type": "sample_kit", "config": { "samples": [
                { "key": 1, "asset": { "path": kick.to_str().unwrap() } }
            ] } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "chance", "to_port": "clock" },
            { "from": "chance", "from_port": gate, "to": "kit", "to_port": "play" },
            { "from": "kit", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
            { "from": "kit", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
        ]
    })
    .to_string()
}

fn render(json: &str, frames: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(json).expect("the invention loads");
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}

#[test]
fn chance_gates_trigger_a_sample_kit_through_the_graph() {
    let kick = temp_wav(0.5, 256);

    // bias 1: gate_1 fires on every clock, so the kit sounds on the first beat.
    let json = kit_invention(
        &kick,
        serde_json::json!({ "bias": 1.0, "seed": 9 }),
        "gate_1",
    );
    let loud = peak(&render(&json, 2_048));
    assert!(
        (loud - 0.5).abs() < 2e-3,
        "expected the kit's 0.5-level sample, peak was {loud}"
    );

    // bias 0: gate_1 never fires, the kit stays silent.
    let json = kit_invention(
        &kick,
        serde_json::json!({ "bias": 0.0, "seed": 9 }),
        "gate_1",
    );
    let silent = peak(&render(&json, 2_048));
    assert!(
        silent < 1e-6,
        "no gate means no kit sound, peak was {silent}"
    );

    // Complementary: at bias 0, gate_2 takes every step gate_1 declines.
    let json = kit_invention(
        &kick,
        serde_json::json!({ "bias": 0.0, "seed": 9 }),
        "gate_2",
    );
    let other = peak(&render(&json, 2_048));
    assert!(
        (other - 0.5).abs() < 2e-3,
        "gate_2 should play at bias 0, peak was {other}"
    );

    let _ = std::fs::remove_file(kick);
}

#[test]
fn chance_frequency_drives_an_oscillator_to_a_note_of_the_scale() {
    let json = serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "chance", "type": "chance", "config": {
                "key": "C", "scale": "major", "steps": 0.5, "spread": 1.0, "seed": 29
            } },
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine" } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "chance", "to_port": "clock" },
            { "from": "chance", "from_port": "frequency_3", "to": "osc", "to_port": "frequency" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio_left" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio_right" }
        ]
    })
    .to_string();

    // The first beat (120 bpm) lasts 24,000 frames; judge the pitch inside it.
    let out = render(&json, 22_000);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    assert!(peak(&left) > 0.1, "the oscillator should be audible");

    let window = &left[2_000..];
    let crossings: Vec<usize> = (1..window.len())
        .filter(|&i| window[i - 1] <= 0.0 && window[i] > 0.0)
        .collect();
    assert!(
        crossings.len() > 20,
        "too few cycles to measure: {}",
        crossings.len()
    );
    let cycles = (crossings.len() - 1) as f64;
    let span = (crossings[crossings.len() - 1] - crossings[0]) as f64;
    let heard = cycles * 48_000.0 / span;
    let midi = midi_of(heard as f32);

    let c_major = &SCALES[0].1;
    let nearest = midi.round();
    assert!(
        (midi - nearest).abs() < 0.25 && in_scale(nearest as i64, 0, c_major),
        "measured {heard} Hz (MIDI {midi}) is not a C major note"
    );
    assert!(
        (47.0..=73.0).contains(&nearest),
        "MIDI {nearest} lies outside low_note 48 .. 48 + range 24"
    );
}
