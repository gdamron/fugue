//! C1 (model order) and C13 (example invention).

use super::rig::*;
use super::*;
use std::path::Path;

/// Criterion C1: "`model` lists `six_op_fm` after `wavetable`; the part-1
/// models and `wavetable` keep their names and order."
/// Assumption: "keep their order" means the seven part-1 models are the first
/// seven options in order and `wavetable` follows them; `six_op_fm` is only
/// required to come after `wavetable` (not to be last, since other options
/// may be appended by parallel work).
#[test]
fn model_lists_six_op_fm_after_wavetable() {
    let models = voice(json!({})).models;
    assert_eq!(models[..7], PART_ONE[..], "part-1 models keep their order");
    let wavetable = model_position("wavetable");
    assert!(wavetable >= 7, "wavetable at {wavetable} in {models:?}");
    let six_op = six_op();
    assert!(
        six_op > wavetable,
        "six_op_fm at {six_op}, wavetable at {wavetable} in {models:?}"
    );
    assert_eq!(models.iter().filter(|m| *m == "six_op_fm").count(), 1);
}

/// Criterion C1 (interface): selectable by position and by name in config,
/// and sounds either way.
/// Assumption: `model` takes the option's name as a string in config.
#[test]
fn six_op_fm_is_selectable_by_position_and_by_config_name() {
    let mut v = voice(json!({}));
    v.select(six_op());
    assert!(peak(&v.render(seconds(0.3))) > 0.01, "selected by position");
    let mut v = voice(json!({ "model": "six_op_fm", "frequency": 220.0 }));
    assert!(peak(&v.render(seconds(0.3))) > 0.01, "selected by config");
}

/// Criterion C13: "An example invention in `examples/` uses
/// `model: "six_op_fm"` with a triggered phrase, loads, and renders audible,
/// finite audio."
/// Assumptions: "triggered" means some connection ends at that module's
/// `trigger` input; audible means a dac peak above 0.01 and RMS above 0.003
/// over 8 s.
#[test]
fn an_example_invention_plays_a_triggered_six_op_fm_phrase() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("examples/ exists") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for m in doc["modules"].as_array().into_iter().flatten() {
            if m["type"] != "macro_oscillator" || m["config"]["model"] != "six_op_fm" {
                continue;
            }
            let patched = doc["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|c| c["to"] == m["id"] && c["to_port"] == "trigger");
            if patched {
                found.push((path.clone(), text.clone()));
            }
        }
    }
    assert!(
        !found.is_empty(),
        "no examples/*.json has a macro_oscillator with model \"six_op_fm\" and a patched trigger"
    );
    for (path, text) in found {
        let name = path.display();
        fugue::Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|err| panic!("{name} loads as an invention: {err}"));
        let mut engine = fugue::RenderEngine::new(SR);
        engine
            .load_json(&text)
            .unwrap_or_else(|err| panic!("{name} loads into the engine: {err}"));
        let mut out = vec![0.0f32; seconds(8.0) * 2];
        engine
            .render_interleaved(&mut out)
            .unwrap_or_else(|err| panic!("{name} renders: {err}"));
        assert!(
            out.iter().all(|s| s.is_finite()),
            "{name}: non-finite audio"
        );
        assert!(
            peak(&out) > 0.01,
            "{name}: peak {} is not audible",
            peak(&out)
        );
        assert!(
            rms(&out) > 0.003,
            "{name}: rms {} is not audible",
            rms(&out)
        );
    }
}
