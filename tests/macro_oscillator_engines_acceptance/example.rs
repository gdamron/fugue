//! Criterion 4: the example invention `macro_oscillator_drum_groove`.

use super::rig::*;
use super::*;
use std::path::PathBuf;

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("macro_oscillator_drum_groove.json")
}

fn text() -> String {
    std::fs::read_to_string(path()).unwrap_or_else(|err| panic!("read {}: {err}", path().display()))
}

/// Criterion: "`examples/macro_oscillator_drum_groove.json` exists [and]
/// loads": it parses as an invention through the public loader.
#[test]
fn the_example_loads_as_an_invention() {
    fugue::Invention::from_file(path().to_str().unwrap())
        .unwrap_or_else(|err| panic!("{} loads as an invention: {err}", path().display()));
}

/// Criterion: "plays a short groove using only the new engines (every
/// macro_oscillator in it uses one of the seven new models)": at least one
/// `macro_oscillator` module, each with a config `model` naming a new engine,
/// and each building through the registry. At least two distinct engines are
/// used, since a groove is more than one voice.
#[test]
fn every_macro_oscillator_in_the_example_uses_a_new_engine() {
    let doc: Value = serde_json::from_str(&text()).expect("the example is JSON");
    let registry = ModuleRegistry::default();
    let mut used = Vec::new();
    for module in doc["modules"].as_array().expect("modules array") {
        if module["type"] != "macro_oscillator" {
            continue;
        }
        let config = module.get("config").cloned().unwrap_or_else(|| json!({}));
        let model = config["model"]
            .as_str()
            .unwrap_or_else(|| panic!("{} has no model name in its config", module["id"]));
        assert!(
            NEW.contains(&model),
            "{} uses model {model}, which is not one of the new engines",
            module["id"]
        );
        registry
            .build("macro_oscillator", SR, &config)
            .unwrap_or_else(|err| panic!("config {config}: {err}"));
        used.push(model.to_string());
    }
    assert!(!used.is_empty(), "the example has no macro_oscillator");
    used.sort();
    used.dedup();
    assert!(used.len() >= 2, "the groove uses only {used:?}");
}

/// Renders `seconds_` of the example's stereo output.
fn render(seconds_: f64) -> Vec<f32> {
    let mut engine = fugue::RenderEngine::new(SR);
    engine
        .load_json(&text())
        .unwrap_or_else(|err| panic!("render engine loads the example: {err}"));
    let mut out = vec![0.0f32; seconds(seconds_) * 2];
    engine.render_interleaved(&mut out).expect("renders");
    out
}

/// Criterion: "its render is not silent": 8 s through the render engine has a
/// peak above 0.05 (about -26 dBFS) and stays finite and within +-1.
/// Assumption: the output is the dac's stereo mix.
#[test]
fn the_example_render_is_not_silent() {
    let out = render(8.0);
    assert!(out.iter().all(|s| s.is_finite()), "non-finite output");
    let p = peak(&out);
    assert!(p > 0.05, "the example's peak is only {p}");
    assert!(p <= 1.0, "the example clips: peak {p}");
}

/// Criterion: "plays a short groove": the 8 s render has at least four
/// onsets, where an onset is a 10 ms frame whose RMS is above -50 dBFS and at
/// least three times that of the frame before it, at least 100 ms after the
/// previous onset.
/// Assumption: a groove with a kick, snare and hat has four or more hits in
/// eight seconds at any musical tempo.
#[test]
fn the_example_plays_a_groove_of_several_hits() {
    let out = render(8.0);
    let mono: Vec<f32> = out.chunks(2).map(|f| 0.5 * (f[0] + f[1])).collect();
    let frame = seconds(0.01);
    let rms: Vec<f32> = mono
        .chunks_exact(frame)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt())
        .collect();
    let mut onsets = Vec::new();
    for k in 1..rms.len() {
        let far_enough = onsets.last().is_none_or(|last| k - last >= 10);
        if rms[k] > 0.003_16 && rms[k] >= 3.0 * rms[k - 1] && far_enough {
            onsets.push(k);
        }
    }
    assert!(
        onsets.len() >= 4,
        "only {} onsets, at 10 ms frames {onsets:?}",
        onsets.len()
    );
}
