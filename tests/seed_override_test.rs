//! The invention seed: every unseeded melody, chance and noise module plays
//! a stream derived from the seed and its module id, and nothing else moves.

use fugue::RenderEngine;
use serde_json::{json, Value};

const SAMPLE_RATE: u32 = 48_000;

/// Each random source with no seed, driving one oscillator or the output.
fn sources(extra: &[Value]) -> Value {
    let mut modules = vec![
        json!({ "id": "clock", "type": "clock", "config": { "bpm": 480.0 } }),
        json!({ "id": "mel", "type": "melody" }),
        json!({ "id": "mel_osc", "type": "oscillator" }),
        json!({ "id": "chance", "type": "chance" }),
        json!({ "id": "chance_osc", "type": "oscillator" }),
        json!({ "id": "noise", "type": "noise", "config": { "level": 0.1 } }),
        json!({ "id": "mixer", "type": "mixer",
                "config": { "channel_count": 3, "levels": [0.3, 0.3, 0.3] } }),
        json!({ "id": "dac", "type": "dac" }),
    ];
    modules.extend_from_slice(extra);
    json!({
        "version": "1.0.0",
        "modules": modules,
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "mel", "to_port": "gate" },
            { "from": "mel", "from_port": "frequency", "to": "mel_osc", "to_port": "frequency" },
            { "from": "clock", "from_port": "beat", "to": "chance", "to_port": "clock" },
            { "from": "chance", "from_port": "frequency_3",
              "to": "chance_osc", "to_port": "frequency" },
            { "from": "mel_osc", "from_port": "audio", "to": "mixer", "to_port": "audio.0" },
            { "from": "chance_osc", "from_port": "audio", "to": "mixer", "to_port": "audio.1" },
            { "from": "noise", "from_port": "white", "to": "mixer", "to_port": "audio.2" },
            { "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
            { "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
        ]
    })
}

/// Two seconds of interleaved stereo.
fn render(document: &Value, seed: Option<u64>) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.set_seed(seed);
    engine.load_json(&document.to_string()).unwrap();
    let mut out = vec![0.0f32; SAMPLE_RATE as usize * 2 * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

#[test]
fn a_seed_repeats_every_unseeded_source_and_another_seed_changes_it() {
    let document = sources(&[]);
    let first = bits(&render(&document, Some(11)));
    assert_eq!(first, bits(&render(&document, Some(11))));
    assert_ne!(first, bits(&render(&document, Some(12))));
    assert_ne!(
        bits(&render(&document, None)),
        bits(&render(&document, None)),
        "unseeded sources still draw from entropy"
    );
}

#[test]
fn each_source_is_seeded_on_its_own() {
    // One source left unseeded at a time: the render repeats only if the
    // invention seed reaches that source.
    for unseeded in ["mel", "chance", "noise"] {
        let mut document = sources(&[]);
        for module in document["modules"].as_array_mut().unwrap() {
            let id = module["id"].as_str().unwrap().to_string();
            if ["mel", "chance", "noise"].contains(&id.as_str()) && id != unseeded {
                module["config"]["seed"] = json!(5);
            }
        }
        let seeded = bits(&render(&document, Some(3)));
        assert_eq!(seeded, bits(&render(&document, Some(3))), "{unseeded}");
        assert_ne!(seeded, bits(&render(&document, Some(4))), "{unseeded}");
    }
}

#[test]
fn a_module_with_its_own_seed_keeps_it() {
    let mut document = sources(&[]);
    for module in document["modules"].as_array_mut().unwrap() {
        if ["mel", "chance", "noise"].contains(&module["id"].as_str().unwrap()) {
            module["config"]["seed"] = json!(9);
        }
    }
    let unseeded = bits(&render(&document, None));
    assert_eq!(unseeded, bits(&render(&document, Some(1))));
    assert_eq!(unseeded, bits(&render(&document, Some(2))));
}

#[test]
fn an_unrelated_module_leaves_the_others_streams_alone() {
    let extra = [
        json!({ "id": "another_melody", "type": "melody" }),
        json!({ "id": "another_noise", "type": "noise" }),
    ];
    let before = bits(&render(&sources(&[]), Some(21)));
    let after = bits(&render(&sources(&extra), Some(21)));
    assert_eq!(before, after);
}

#[test]
fn instances_of_a_development_play_their_own_streams() {
    let document = json!({
        "version": "1.0.0",
        "developments": [{
            "name": "hiss",
            "definition": {
                "version": "1.0.0",
                "modules": [{ "id": "noise", "type": "noise", "config": { "level": 0.1 } }],
                "connections": [],
                "outputs": [{ "name": "audio", "from": "noise", "from_port": "white" }]
            }
        }],
        "modules": [
            { "id": "left", "type": "hiss" },
            { "id": "right", "type": "hiss" },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "left", "from_port": "audio", "to": "dac", "to_port": "audio_left" },
            { "from": "right", "from_port": "audio", "to": "dac", "to_port": "audio_right" }
        ]
    });
    let out = render(&document, Some(8));
    assert_eq!(bits(&out), bits(&render(&document, Some(8))));
    let left: Vec<u32> = out.iter().step_by(2).map(|s| s.to_bits()).collect();
    let right: Vec<u32> = out.iter().skip(1).step_by(2).map(|s| s.to_bits()).collect();
    assert_ne!(left, right, "the two instances play one stream");
}
