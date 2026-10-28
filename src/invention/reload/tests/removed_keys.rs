//! A key the file stated and a reload's file removes: it returns, by a
//! control write, to the value a fresh build of the new file plays, as a key
//! the file never stated does, so deleting a setting glides back to its
//! default rather than rebuilding the module. A key no write reaches still
//! rebuilds.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn start_pumped(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    start_manual(runtime)
}

/// One module `m` of type `module_type` configured by `config`, into the dac
/// from its `port`.
fn single(module_type: &str, config: serde_json::Value, port: &str) -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "m", "type": module_type, "config": config },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "m", "from_port": port, "to": "dac", "to_port": "audio" }]
    })
    .to_string()
}

/// Loads `from`, reloads `to`, and returns the report with `m.key` after it.
fn reload(from: &str, to: &str, key: &str) -> (ReloadReport, ControlValue) {
    let (mut running, pump) = start_pumped(from);
    pump.render(1);
    let report = running.reload(doc(to)).expect("diff applies");
    pump.render(1);
    (report, running.get_control("m", key).unwrap())
}

/// Removing `key` from `config` returns `m.key` to `restored` by one control
/// write, keeping the module's instance.
fn glides_back(module_type: &str, config: serde_json::Value, key: &str, restored: f32) {
    let mut removed = config.clone();
    removed.as_object_mut().unwrap().remove(key);
    let port = if module_type == "mixer" {
        "audio_left"
    } else {
        "audio"
    };
    let (report, value) = reload(
        &single(module_type, config, port),
        &single(module_type, removed, port),
        key,
    );
    assert!(report.swapped.is_empty(), "{key}: {report:?}");
    assert_eq!(report.controls_updated, [format!("m.{key}")], "{report:?}");
    assert_eq!(value, number(restored), "{key}");
}

#[test]
fn a_removed_scalar_glides_back_to_its_default() {
    let tone = json!({ "waveform": "sine", "frequency": 220.0 });
    glides_back("oscillator", tone, "frequency", 440.0);
    let filtered = json!({ "cutoff": 300.0, "resonance": 0.5 });
    glides_back("filter", filtered, "cutoff", fresh("filter", "cutoff"));
}

#[test]
fn a_removed_indexed_key_glides_back_to_what_the_file_builds() {
    glides_back(
        "mixer",
        json!({ "channel_count": 2, "level.1": 0.3 }),
        "level.1",
        1.0,
    );
    // Within a whole array, the index returns to the array's value.
    let levels = json!({ "channel_count": 2, "levels": [0.8, 0.6], "level.1": 0.3 });
    glides_back("mixer", levels, "level.1", 0.6);
}

#[test]
fn a_removed_key_no_write_reaches_still_rebuilds() {
    // A mixer's channel count is read-only at runtime.
    let from = single("mixer", json!({ "channel_count": 2 }), "audio_left");
    let to = single("mixer", json!({}), "audio_left");
    let (report, count) = reload(&from, &to, "channel_count");
    assert_eq!(report.swapped, ["m"], "{report:?}");
    assert_eq!(count, fresh_control(&to, "channel_count"));
}

#[test]
fn a_removed_key_of_a_development_instance_still_rebuilds() {
    let line = |config: &str| {
        format!(
            r#"{{
            "version": "1.0.0",
            "developments": [{{ "name": "line", "definition": {{
                "version": "1.0.0",
                "modules": [{{ "id": "a", "type": "oscillator", "config": {{ "waveform": "sine" }} }}],
                "connections": [],
                "outputs": [{{ "name": "out", "from": "a", "from_port": "audio" }}],
                "controls": [{{ "name": "pitch", "module": "a", "control": "frequency" }}]
            }} }}],
            "modules": [
                {{ "id": "m", "type": "line", "config": {config} }},
                {{ "id": "dac", "type": "dac" }}
            ],
            "connections": [{{ "from": "m", "from_port": "out", "to": "dac", "to_port": "audio" }}]
        }}"#
        )
    };
    let (report, pitch) = reload(&line(r#"{ "pitch": 220.0 }"#), &line("{}"), "pitch");
    assert_eq!(report.swapped, ["m"], "{report:?}");
    assert_eq!(pitch, number(440.0));
}

/// What `key` of module `m` holds in a fresh build of `json`.
fn fresh_control(json: &str, key: &str) -> ControlValue {
    let (running, _pump) = start_pumped(json);
    running.get_control("m", key).unwrap()
}

/// A module type's default for `key`, as a fresh build with no config holds it.
fn fresh(module_type: &str, key: &str) -> f32 {
    match fresh_control(&single(module_type, json!({}), "audio"), key) {
        ControlValue::Number(value) => value,
        other => panic!("{key} is {other:?}"),
    }
}

#[test]
fn a_removed_sample_still_rebuilds() {
    // No write unloads a sample: an empty asset is how a player built
    // without one reads.
    let dir = tempfile::tempdir().unwrap();
    let sample = crate::test_support::sampler::wav(dir.path(), "a.wav", 441);
    let from = single("sample_player", json!({ "asset": sample }), "audio_left");
    let to = single("sample_player", json!({}), "audio_left");
    let (report, asset) = reload(&from, &to, "asset");
    assert_eq!(report.swapped, ["m"], "{report:?}");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(asset, ControlValue::String(String::new()));
}

#[test]
fn a_removed_weight_returns_by_the_whole_weights_and_leaves_no_override() {
    let tune = |degrees: &str, weight: Option<f32>| {
        let mut config = json!({
            "degrees": degrees, "degree_count": 4, "note_weights": "[1, 2]", "seed": 1
        });
        if let Some(weight) = weight {
            config["note_weight.3"] = json!(weight);
        }
        single("melody", config, "frequency")
    };
    let (mut running, pump) = start_pumped(&tune("[0, 2, 4]", Some(9.0)));
    pump.render(1);
    let removed = tune("[0, 2, 4]", None);
    let report = running.reload(doc(&removed)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["m.note_weights"], "{report:?}");
    let weight = || running.get_control("m", "note_weight.3").unwrap();
    assert_eq!(weight(), fresh_control(&removed, "note_weight.3"));

    // A whole scale keeps a weight written to its position: the removed
    // one was not, so the new scale weights it as a fresh build does.
    let rescaled = tune("[0, 7]", None);
    let report = running.reload(doc(&rescaled)).expect("diff applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    let weight = running.get_control("m", "note_weight.3").unwrap();
    assert_eq!(weight, fresh_control(&rescaled, "note_weight.3"));
}
