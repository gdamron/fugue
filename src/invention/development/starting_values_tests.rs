//! A development instance starts at its configured values: its first block
//! already plays them, with no glide in from the inner modules' defaults.

use serde_json::{json, Value};

use crate::RenderEngine;

/// The left channel of the first `frames` frames of `invention`.
fn render(invention: &Value, frames: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(&invention.to_string()).unwrap();
    let mut out = vec![0.0; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out.chunks(2).map(|frame| frame[0]).collect()
}

/// A development passing a still square wave (a constant 1 for the first
/// seconds) through `chain`, each entry `(module, input port, output
/// port)`, with `controls` exposed; one instance configured with `config`,
/// into the dac.
fn development(chain: &[(Value, &str, &str)], controls: Value, config: Value) -> Value {
    let mut modules = vec![json!({
        "id": "osc", "type": "oscillator",
        "config": { "frequency": 0.001, "waveform": "square" }
    })];
    let mut connections = Vec::new();
    let mut from = ("osc".to_string(), "audio");
    for (module, input, output) in chain {
        let id = module["id"].as_str().unwrap().to_string();
        connections.push(json!({
            "from": from.0, "from_port": from.1, "to": id, "to_port": input
        }));
        modules.push(module.clone());
        from = (id, output);
    }
    json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": {
            "version": "1.0.0",
            "modules": modules,
            "connections": connections,
            "outputs": [{ "name": "audio", "from": from.0, "from_port": from.1 }],
            "controls": controls
        }}],
        "modules": [
            { "id": "v", "type": "voice", "config": config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [{ "from": "v", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    })
}

/// Every frame of `out` is `expected`, to within rounding.
fn assert_steady(out: &[f32], expected: f32) {
    for (i, &x) in out.iter().enumerate() {
        assert!(
            (x - expected).abs() < 1e-5,
            "frame {i}: {x}, not {expected}"
        );
    }
}

#[test]
fn gliding_controls_start_at_the_configured_values() {
    let invention = development(
        &[
            (json!({ "id": "vca", "type": "vca" }), "audio", "audio"),
            (json!({ "id": "gain", "type": "gain" }), "audio", "audio"),
        ],
        json!([
            { "name": "level", "module": "vca", "control": "level" },
            { "name": "decibels", "module": "gain", "control": "decibels" }
        ]),
        json!({ "level": 0.25, "decibels": -12.0 }),
    );
    let expected = 0.25 * crate::dsp::decibels_to_level(-12.0);
    assert_steady(&render(&invention, 1_024), expected);
}

#[test]
fn a_position_of_an_array_starts_at_its_value() {
    let mixer = |level: f32| {
        json!({
            "id": "mix", "type": "mixer",
            "config": { "channel_count": 1, "levels": [level], "pans": [0.0], "master": 1.0 }
        })
    };
    let controls = json!([{ "name": "level", "module": "mix", "control": "level.0" }]);
    let configured = development(
        &[(mixer(1.0), "audio.0", "audio_left")],
        controls.clone(),
        json!({ "level": 0.5 }),
    );
    let written = development(
        &[(mixer(0.5), "audio.0", "audio_left")],
        controls,
        json!({}),
    );
    assert_eq!(render(&configured, 1_024), render(&written, 1_024));
}

#[test]
fn a_nested_development_starts_at_the_value_passed_down() {
    let mut invention = development(
        &[(json!({ "id": "nest", "type": "inner" }), "audio", "audio")],
        json!([{ "name": "level", "module": "nest", "control": "level" }]),
        json!({ "level": 0.25 }),
    );
    invention["developments"][0]["definition"]["developments"] = json!([{
        "name": "inner",
        "definition": {
            "version": "1.0.0",
            "modules": [{ "id": "vca", "type": "vca" }],
            "connections": [],
            "inputs": [{ "name": "audio", "to": "vca", "to_port": "audio" }],
            "outputs": [{ "name": "audio", "from": "vca", "from_port": "audio" }],
            "controls": [{ "name": "level", "module": "vca", "control": "level" }]
        }
    }]);
    assert_steady(&render(&invention, 1_024), 0.25);
}

#[test]
fn a_value_the_inner_config_refuses_still_fails_the_build() {
    let invention = development(
        &[(json!({ "id": "vca", "type": "vca" }), "audio", "audio")],
        json!([{ "name": "level", "module": "vca", "control": "level" }]),
        json!({ "level": "loud" }),
    );
    let mut engine = RenderEngine::new(48_000);
    assert!(engine.load_json(&invention.to_string()).is_err());
}
