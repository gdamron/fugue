//! A development instance's config holds the controls its development
//! exposes: a key named `asset` is read as an audio asset only when the
//! control exposed under it is an inner module's asset, on load and on
//! reload alike.

use serde_json::json;

use super::sample_levels::{constant_wav, start_pumped};
use super::*;

/// An invention saved at `dir/piece.json` whose instance `m` of a
/// development exposes `asset` as `inner`'s control `control` and its
/// `port` as `out`, configured with `config`.
fn piece(
    dir: &std::path::Path,
    (inner, port): (serde_json::Value, &str),
    control: &str,
    config: serde_json::Value,
) -> Invention {
    let path = dir.join("piece.json");
    let invention = json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": {
            "version": "1.0.0",
            "modules": [inner],
            "connections": [],
            "outputs": [{ "name": "out", "from": "a", "from_port": port }],
            "controls": [{ "name": "asset", "module": "a", "control": control }]
        } }],
        "modules": [
            { "id": "m", "type": "voice", "config": config },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "m", "from_port": "out", "to": "dac", "to_port": "audio" }]
    });
    Invention::from_json(&invention.to_string())
        .unwrap()
        .save_to_file(&path)
        .unwrap();
    Invention::from_file(&path.to_string_lossy()).unwrap()
}

#[test]
fn an_exposed_asset_that_names_no_audio_is_not_read_as_one() {
    let dir = tempfile::tempdir().unwrap();
    let tone = json!({ "id": "a", "type": "oscillator", "config": { "waveform": "sine" } });
    let at = |pitch: f32| {
        piece(
            dir.path(),
            (tone.clone(), "audio"),
            "frequency",
            json!({ "asset": pitch }),
        )
    };
    let (mut running, pump) = start_pumped(at(330.0));
    assert_eq!(running.get_control("m", "asset").unwrap(), 330.0.into());
    let report = running.reload(at(220.0)).expect("the reload applies");
    pump.render(1);
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(running.get_control("m", "asset").unwrap(), 220.0.into());
}

#[test]
fn an_exposed_asset_that_names_an_inner_sample_still_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let wav = constant_wav(dir.path(), 0.5, 4_800);
    let player = json!({ "id": "a", "type": "sample_player", "config": {} });
    let relative = json!({ "asset": "level-0.5.wav" });
    let document = piece(dir.path(), (player, "audio_left"), "asset", relative);
    let (mut running, _pump) = start_pumped(document.clone());
    let loaded = running.get_control("m", "asset").unwrap();
    let ControlValue::String(loaded) = loaded else {
        panic!("an asset control holds a path");
    };
    assert_eq!(std::path::Path::new(&loaded), std::path::Path::new(&wav));
    let report = running.reload(document).expect("the reload applies");
    assert!(report.swapped.is_empty(), "{report:?}");
}

#[test]
fn an_added_instance_reads_its_exposed_keys_as_declared() {
    let dir = tempfile::tempdir().unwrap();
    let tone = json!({ "id": "a", "type": "oscillator", "config": { "waveform": "sine" } });
    let (mut running, _pump) =
        start_pumped(piece(dir.path(), (tone, "audio"), "frequency", json!({})));
    let add = crate::rpc::StructuralEdit::AddModule {
        id: "n".into(),
        module_type: "voice".into(),
        config: json!({ "asset": 330.0 }),
    };
    running.apply_edits(&[add]).expect("the batch applies");
    assert_eq!(running.get_control("n", "asset").unwrap(), 330.0.into());
}

#[test]
fn saving_elsewhere_leaves_an_exposed_asset_that_names_no_audio_as_written() {
    let dir = tempfile::tempdir().unwrap();
    let tone = json!({ "id": "a", "type": "oscillator", "config": {} });
    let shaped = json!({ "asset": "square" });
    let document = piece(dir.path(), (tone, "audio"), "waveform", shaped);
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("copy.json");
    document.save_to_file(&target).unwrap();
    let saved = Invention::from_file(&target.to_string_lossy()).unwrap();
    assert_eq!(saved.modules[0].config["asset"], "square");
    let (running, _pump) = start_pumped(saved);
    let shape = running.get_control("m", "asset").unwrap();
    assert_eq!(shape, ControlValue::String("square".into()));
}

#[test]
fn only_saving_elsewhere_is_refused_when_an_instances_declaration_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let tone = json!({ "id": "a", "type": "oscillator", "config": {} });
    let shaped = json!({ "asset": "square" });
    let mut document = piece(dir.path(), (tone, "audio"), "waveform", shaped);
    let gone = json!({ "name": "gone", "path": "gone.json" });
    document
        .developments
        .push(serde_json::from_value(gone).unwrap());
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("copy.json");
    let error = document.save_to_file(&target).unwrap_err().to_string();
    assert!(error.contains("does not load"), "{error}");
    assert!(!target.exists());
    let beside = dir.path().join("piece.json");
    document
        .save_to_file(&beside)
        .expect("saving beside the source applies");
    let saved = Invention::from_file(&beside.to_string_lossy()).unwrap();
    assert_eq!(saved.modules[0].config["asset"], "square");
}
