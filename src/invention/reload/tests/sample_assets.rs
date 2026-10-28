//! A sample written to a kit slot's or an instrument zone's `asset.N` is
//! recorded into the entry it names, so it plays after a load of the saved
//! document, a reload of it, a reload that rebuilds the module and an edit
//! batch, and the audio-asset scan finds it.

use serde_json::json;

use super::sample_levels::{constant_wav, document, saved, start_pumped};
use super::*;
use crate::invention::manual_backend::Pump;
use crate::rpc::StructuralEdit;
use crate::ControlWriteIntent;

const TOL: f32 = 1e-3;

/// Sounds the first slot or zone and returns the loudest left-channel
/// sample, as `sample_levels::peak` does, but as a performance: a recorded
/// note would be replayed by every reload of the saved document.
fn peak(running: &RunningInvention, pump: &Pump, sound: (&str, f32)) -> f32 {
    let play = |key: &str| {
        let value = ControlValue::Number(sound.1);
        (running.snapshot())
            .set_control_with_intent("m", key, value, ControlWriteIntent::Perform)
            .unwrap()
    };
    play(sound.0);
    if sound.0 == "play" {
        // Past the fade of the play it cut off, as there.
        pump.render(3);
    }
    let peak = pump
        .render(4)
        .iter()
        .fold(0.0f32, |acc, s| acc.max(s.abs()));
    if sound.0 == "note_on" {
        play("note_off");
        pump.render(200);
    }
    peak
}

/// The sampler `m` playing `asset` from its first entry, and how to sound it.
fn sampler(type_id: &str, asset: &str) -> (serde_json::Value, &'static str, (&'static str, f32)) {
    match type_id {
        "sample_kit" => (
            json!({ "type": type_id, "config": { "samples": [{ "key": 36, "asset": asset }] } }),
            "samples",
            ("play", 36.0),
        ),
        _ => (
            json!({ "type": type_id, "config": {
                "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": asset }]
            } }),
            "zones",
            ("note_on", 60.0),
        ),
    }
}

/// Writes `asset.0` from a sample at 0.5 to one at 0.25, then checks the
/// written sample is what plays after each way the module can be built again.
fn written_asset_survives(type_id: &str) {
    let dir = tempfile::tempdir().unwrap();
    let original = constant_wav(dir.path(), 0.5, 48_000);
    let written = constant_wav(dir.path(), 0.25, 48_000);
    let (module, entries, sound) = sampler(type_id, &original);
    let (mut running, pump) = start_pumped(document(module));
    assert!((peak(&running, &pump, sound) - 0.5).abs() < TOL);
    running
        .set_control("m", "asset.0", ControlValue::String(written.clone()))
        .unwrap();
    assert!((peak(&running, &pump, sound) - 0.25).abs() < TOL);

    // Recorded in the entry, where the build and the asset scan read it.
    let mut document = saved(&running);
    let config = &mut document.modules[0].config;
    assert_eq!(config[entries][0]["asset"], json!(written), "{config}");
    assert!(config.get("asset.0").is_none(), "{config}");
    let scanned: Vec<_> = crate::invention::audio_assets::module_asset_values(config)
        .map(|value| value.clone())
        .collect();
    assert_eq!(scanned, [json!(written)]);

    // Saved and loaded cold.
    let (loaded, loaded_pump) = start_pumped(saved(&running));
    assert_eq!(
        loaded.get_control("m", "asset.0").unwrap(),
        ControlValue::String(written.clone())
    );
    assert!((peak(&loaded, &loaded_pump, sound) - 0.25).abs() < TOL);

    // A reload of the saved document is no change.
    let report = running.reload(saved(&running)).expect("the reload applies");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.25).abs() < TOL);

    // A reload whose config change rebuilds the module.
    let mut edited = saved(&running);
    let config = &mut edited.modules[0].config;
    let extra = config[entries][0].clone();
    config[entries].as_array_mut().unwrap().push(extra);
    if entries == "samples" {
        config[entries][1]["key"] = json!(99);
    }
    let report = running.reload(edited).expect("the reload applies");
    assert_eq!(report.swapped, ["m"], "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.25).abs() < TOL);

    // A reload of the original file plays its sample again.
    let (module, _, _) = sampler(type_id, &original);
    let report = running
        .reload(super::sample_levels::document(module))
        .unwrap();
    assert_eq!(report.swapped, ["m"], "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.5).abs() < TOL);
}

#[test]
fn a_kit_slots_written_asset_survives_a_load_and_a_rebuild() {
    written_asset_survives("sample_kit");
}

#[test]
fn an_instrument_zones_written_asset_survives_a_load_and_a_rebuild() {
    written_asset_survives("sample_instrument");
}

/// An edit batch's `asset.N` write is a control write on the running module,
/// not a rebuild the batch never asked for.
#[test]
fn an_edit_batch_writes_a_slots_asset_without_rebuilding() {
    for type_id in ["sample_kit", "sample_instrument"] {
        let dir = tempfile::tempdir().unwrap();
        let original = constant_wav(dir.path(), 0.5, 48_000);
        let written = constant_wav(dir.path(), 0.25, 48_000);
        let (module, entries, sound) = sampler(type_id, &original);
        let (mut running, pump) = start_pumped(document(module));
        let report = running
            .apply_edits(&[StructuralEdit::SetControl {
                module_id: "m".into(),
                key: "asset.0".into(),
                value: ControlValue::String(written.clone()),
            }])
            .expect("the batch applies");
        assert!(
            report.rebuilt.is_empty() && report.controls_failed.is_empty(),
            "{type_id}: {report:?}"
        );
        assert!(
            (peak(&running, &pump, sound) - 0.25).abs() < TOL,
            "{type_id}"
        );
        let document = saved(&running);
        assert_eq!(
            document.modules[0].config[entries][0]["asset"],
            json!(written)
        );

        let (loaded, loaded_pump) = start_pumped(document);
        assert!(
            (peak(&loaded, &loaded_pump, sound) - 0.25).abs() < TOL,
            "{type_id}"
        );
    }
}

/// A top-level `asset.N` an older save recorded gives way to the entry.
#[test]
fn a_write_drops_a_top_level_asset_key_an_older_save_recorded() {
    let mut config = json!({ "samples": [{ "key": 36, "asset": "a.wav" }], "asset.0": "b.wav" });
    crate::invention::authored_document::write_config_control(
        &mut config,
        "asset.0",
        &ControlValue::String("c.wav".into()),
    );
    assert_eq!(
        config,
        json!({ "samples": [{ "key": 36, "asset": "c.wav" }] })
    );
}

/// A batch's sample that does not load fails its write at commit, not the
/// validation build (see the batch tests for its other edits committing):
/// the slot keeps the sample it had, in the module and in the document.
#[test]
fn a_batch_sample_that_does_not_load_keeps_the_slots_sample() {
    let dir = tempfile::tempdir().unwrap();
    let original = constant_wav(dir.path(), 0.5, 48_000);
    let (module, entries, sound) = sampler("sample_kit", &original);
    let (mut running, pump) = start_pumped(document(module));
    let report = running
        .apply_edits(&[StructuralEdit::SetControl {
            module_id: "m".into(),
            key: "asset.0".into(),
            value: ControlValue::String("/no/such/sample.wav".into()),
        }])
        .expect("the batch commits");
    assert_eq!(report.controls_failed.len(), 1, "{report:?}");
    assert!(report.rebuilt.is_empty(), "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.5).abs() < TOL);
    let document = saved(&running);
    assert_eq!(
        document.modules[0].config[entries][0]["asset"],
        json!(original)
    );
}

/// An older save that recorded `asset.N` at the top level as well as in the
/// entry: a write replaces both with the entry, a reload of the saved
/// document keeps the module, and a batch's write is no rebuild.
#[test]
fn a_write_over_an_older_saves_top_level_asset_keeps_the_module() {
    let dir = tempfile::tempdir().unwrap();
    let original = constant_wav(dir.path(), 0.5, 48_000);
    let written = constant_wav(dir.path(), 0.25, 48_000);
    let (mut module, entries, sound) = sampler("sample_kit", &original);
    module["config"]["asset.0"] = json!(original);

    let (mut running, pump) = start_pumped(document(module.clone()));
    running
        .set_control("m", "asset.0", ControlValue::String(written.clone()))
        .unwrap();
    // Recorded once it applies.
    pump.block();
    let config = saved(&running).modules[0].config.clone();
    assert!(config.get("asset.0").is_none(), "{config}");
    assert_eq!(config[entries][0]["asset"], json!(written));
    let report = running.reload(saved(&running)).expect("the reload applies");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.25).abs() < TOL);

    let (mut running, pump) = start_pumped(document(module));
    let report = running
        .apply_edits(&[StructuralEdit::SetControl {
            module_id: "m".into(),
            key: "asset.0".into(),
            value: ControlValue::String(written.clone()),
        }])
        .expect("the batch applies");
    assert!(report.rebuilt.is_empty(), "{report:?}");
    assert!((peak(&running, &pump, sound) - 0.25).abs() < TOL);
}

/// A slot whose entry is a `$asset` reference may hold only the reference,
/// so its write is recorded at the top level and the saved document still
/// loads.
#[test]
fn a_write_to_a_referenced_entry_leaves_the_reference_whole() {
    let dir = tempfile::tempdir().unwrap();
    let original = constant_wav(dir.path(), 0.5, 48_000);
    let written = constant_wav(dir.path(), 0.25, 48_000);
    let slot = dir.path().join("slot.json");
    std::fs::write(&slot, json!({ "key": 36, "asset": original }).to_string()).unwrap();
    let mut root = document(json!({ "type": "sample_kit", "config": {
        "samples": [{ "$asset": "slot" }]
    } }));
    root.assets.insert(
        "slot".into(),
        crate::invention::format::AssetSpec {
            path: "slot.json".into(),
        },
    );
    let path = dir.path().join("root.json");
    std::fs::write(&path, root.to_json().unwrap()).unwrap();
    let load = || Invention::from_file(path.to_str().unwrap()).unwrap();

    let (running, pump) = start_pumped(load());
    running
        .set_control("m", "asset.0", ControlValue::String(written.clone()))
        .unwrap();
    assert!((peak(&running, &pump, ("play", 36.0)) - 0.25).abs() < TOL);
    let config = &running.document().unwrap().modules[0].config;
    assert_eq!(
        config["samples"][0],
        json!({ "$asset": "slot" }),
        "{config}"
    );
    assert_eq!(config["asset.0"], json!(written), "{config}");

    std::fs::write(&path, running.document().unwrap().to_json().unwrap()).unwrap();
    start_pumped(load());
}

/// A package ref written to a slot is stored resolved, as the load resolves
/// it, so a reload of the saved document naming the ref keeps the module.
#[test]
fn a_written_package_ref_reloads_without_rebuilding() {
    let dir = tempfile::tempdir().unwrap();
    let original = constant_wav(dir.path(), 0.5, 48_000);
    let package = dir.path().join("packs/fugue.test.kit/1.0.0");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("fugue.pkg.json"), "{}").unwrap();
    let written = constant_wav(&package, 0.25, 48_000);
    let file = std::path::Path::new(&written).file_name().unwrap();
    let reference = format!("fugue.test.kit@1.0.0:{}", file.to_str().unwrap());
    crate::pkg::audio_asset::with_packs_dir(&dir.path().join("packs"), || {
        for type_id in ["sample_kit", "sample_instrument"] {
            let (module, entries, sound) = sampler(type_id, &original);
            let (mut running, pump) = start_pumped(document(module));
            running
                .set_control("m", "asset.0", ControlValue::String(reference.clone()))
                .unwrap();
            pump.block();
            let document = saved(&running);
            assert_eq!(
                document.modules[0].config[entries][0]["asset"],
                json!(reference)
            );
            let report = running.reload(document).expect("the reload applies");
            assert!(report.swapped.is_empty(), "{type_id}: {report:?}");
            assert!(
                (peak(&running, &pump, sound) - 0.25).abs() < TOL,
                "{type_id}"
            );
        }
    });
}
