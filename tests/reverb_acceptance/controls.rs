//! Criterion 5: the control set, factory and an invention using them.

use super::rig::*;
use super::*;

const KEYS: [&str; 7] = [
    "room_size",
    "decay",
    "damping",
    "wet",
    "dry",
    "width",
    "freeze",
];

/// Criterion 5: numbers clamp to 0..1; freeze is on above 0.5.
#[test]
fn controls_clamp_and_freeze_thresholds() {
    let mut reverb = Reverb::new(SR);
    for key in &KEYS[..6] {
        reverb.set_control(key, 7.5).unwrap();
        assert_eq!(reverb.get_control(key).unwrap(), 1.0, "{key} clamps high");
        reverb.set_control(key, -3.0).unwrap();
        assert_eq!(reverb.get_control(key).unwrap(), 0.0, "{key} clamps low");
        reverb.set_control(key, 0.25).unwrap();
        assert_eq!(
            reverb.get_control(key).unwrap(),
            0.25,
            "{key} keeps in-range values"
        );
    }
    for (written, expected) in [(0.0, 0.0), (0.5, 0.0), (0.51, 1.0), (1.0, 1.0), (0.2, 0.0)] {
        reverb.set_control("freeze", written).unwrap();
        assert_eq!(
            reverb.get_control("freeze").unwrap(),
            expected,
            "freeze written {written}"
        );
    }
}

/// Criterion 5: an existing invention using every reverb key loads and plays
/// through the engine: oscillator -> reverb -> dac, audible and finite.
#[test]
fn invention_with_reverb_loads_and_plays() {
    let doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 330.0 } },
            { "id": "verb", "type": "reverb", "config": {
                "room_size": 0.85, "decay": 0.5, "damping": 0.45, "wet": 0.5,
                "dry": 0.5, "width": 1.0, "freeze": false } },
            { "id": "dac", "type": "dac", "config": {} }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "verb", "to_port": "audio_left" },
            { "from": "verb", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
            { "from": "verb", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
        ]
    });
    let mut engine = RenderEngine::new(SR);
    engine
        .load_json(&doc.to_string())
        .expect("invention with reverb loads");
    let mut out = vec![0.0f32; secs(1.0) * 2];
    engine.render_interleaved(&mut out).unwrap();
    assert!(all_finite(&out));
    assert!(rms(&out) > 1e-3, "the invention is audible");
}
