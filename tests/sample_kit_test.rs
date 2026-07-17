//! End-to-end graph test for the sample_kit module: an invention JSON is
//! loaded through the same builder the live daemon uses, a clock's gate
//! triggers a kit slot, and the mixed audio reaches the DAC.

use fugue::RenderEngine;
use std::path::PathBuf;

fn temp_wav(level: f32, frames: usize) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("fugue-kit-e2e-{nanos}-{level}.wav"));
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

#[test]
fn clock_gate_triggers_kit_slot_through_graph() {
    let kick = temp_wav(0.5, 256);
    let json = format!(
        r#"{{
            "version": "1.0.0",
            "modules": [
                {{ "id": "clock", "type": "clock", "config": {{ "bpm": 120.0 }} }},
                {{ "id": "kit", "type": "sample_kit", "config": {{ "samples": [
                    {{ "key": 1, "asset": {{ "path": "{kick_path}" }} }}
                ] }} }},
                {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
            ],
            "connections": [
                {{ "from": "clock", "from_port": "gate", "to": "kit", "to_port": "trigger" }},
                {{ "from": "kit", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" }},
                {{ "from": "kit", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }}
            ]
        }}"#,
        kick_path = kick.to_str().unwrap().replace('\\', "\\\\"),
    );

    let mut engine = RenderEngine::new(48_000);
    engine.load_json(&json).unwrap();
    let mut out = vec![0.0f32; 1024 * 2];
    engine.render_interleaved(&mut out).unwrap();

    // The clock's first gate rise (value 1.0 = slot key 1) starts the kick:
    // the render must contain the sample's 0.5 level on both channels.
    let peak = out.iter().fold(0.0f32, |acc, s| acc.max(*s));
    assert!(
        (peak - 0.5).abs() < 2e-3,
        "expected the kit's 0.5-level sample in the render, peak was {peak}"
    );

    let _ = std::fs::remove_file(kick);
}
