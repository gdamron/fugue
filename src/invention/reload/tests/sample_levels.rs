//! A level written to a sample kit slot or an instrument zone survives a
//! reload of the saved document and a reload that rebuilds the module
//! (FUG-341): the rebuilt module starts at the written level, not the
//! file's.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

/// A mono WAV holding `frames` frames at a constant `level`.
fn constant_wav(dir: &std::path::Path, level: f32, frames: usize) -> String {
    let path = dir.join(format!("level-{level}.wav"));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
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
    path.to_string_lossy().into_owned()
}

/// `module` (the sampler, id `m`) feeding the dac.
fn document(module: serde_json::Value) -> Invention {
    let mut module = module;
    module["id"] = json!("m");
    Invention::from_json(
        &json!({
            "version": "1.0.0",
            "modules": [module, { "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
            "connections": [
                { "from": "m", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
                { "from": "m", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
            ]
        })
        .to_string(),
    )
    .unwrap()
}

fn start_pumped(document: Invention) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    start_manual(runtime)
}

/// The retained document as saving writes it and loading reads it back.
fn saved(running: &RunningInvention) -> Invention {
    let json = running.document().unwrap().to_json().unwrap();
    Invention::from_json(&json).unwrap()
}

/// Sounds the first slot or zone with `sound` and returns the loudest
/// left-channel sample over the next blocks. An instrument's note is then
/// released and left to fade, so the next note starts from silence.
fn peak(running: &RunningInvention, pump: &Pump, sound: (&str, f32)) -> f32 {
    let play = |key: &str| {
        running
            .set_control("m", key, ControlValue::Number(sound.1))
            .unwrap()
    };
    play(sound.0);
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

/// Writes `level.0`, then checks the written level is what plays after a
/// cold load of the saved document and after a reload that rebuilds `m`
/// (`entries` gains a second entry).
fn written_level_survives(module: serde_json::Value, entries: &str, sound: (&str, f32)) {
    const TOL: f32 = 1e-3;
    let (mut running, pump) = start_pumped(document(module));
    assert!((peak(&running, &pump, sound) - 0.5).abs() < TOL);
    running
        .set_control("m", "level.0", ControlValue::Number(0.25))
        .unwrap();
    assert!((peak(&running, &pump, sound) - 0.125).abs() < TOL);

    // Saved and loaded cold.
    let (loaded, loaded_pump) = start_pumped(saved(&running));
    assert_eq!(
        loaded.get_control("m", "level.0").unwrap(),
        ControlValue::Number(0.25)
    );
    assert!((peak(&loaded, &loaded_pump, sound) - 0.125).abs() < TOL);

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
    assert_eq!(
        running.get_control("m", "level.0").unwrap(),
        ControlValue::Number(0.25)
    );
    assert!((peak(&running, &pump, sound) - 0.125).abs() < TOL);
}

#[test]
fn a_kit_slots_written_level_survives_a_load_and_a_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let asset = constant_wav(dir.path(), 0.5, 48_000);
    written_level_survives(
        json!({ "type": "sample_kit", "config": { "samples": [{ "key": 36, "asset": asset }] } }),
        "samples",
        ("play", 36.0),
    );
}

#[test]
fn an_instrument_zones_written_level_survives_a_load_and_a_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let asset = constant_wav(dir.path(), 0.5, 48_000);
    written_level_survives(
        json!({ "type": "sample_instrument", "config": {
            "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": asset }]
        } }),
        "zones",
        ("note_on", 60.0),
    );
}

#[test]
fn a_fixed_key_or_root_note_in_config_is_refused_as_a_write_is() {
    let dir = tempfile::tempdir().unwrap();
    let asset = constant_wav(dir.path(), 0.5, 64);
    let registry = crate::ModuleRegistry::default();
    let cases = [
        (
            "sample_kit",
            json!({ "samples": [{ "key": 36, "asset": asset }], "key.0": 38 }),
            "config 'key.0': Slot keys are fixed at build time",
        ),
        (
            "sample_instrument",
            json!({ "zones": [{ "root_note": 60, "asset": asset }], "root_note.0": 62 }),
            "config 'root_note.0': Zone roots and ranges are fixed at build time",
        ),
    ];
    for (type_id, config, refusal) in cases {
        let error = registry
            .build(type_id, SAMPLE_RATE, &config)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(refusal), "{error}");
    }
}

#[test]
fn a_module_built_with_a_written_level_plays_without_allocating_or_freeing() {
    let dir = tempfile::tempdir().unwrap();
    let asset = constant_wav(dir.path(), 0.5, 4_800);
    let registry = crate::ModuleRegistry::default();
    let cases = [
        (
            "sample_kit",
            json!({ "samples": [{ "key": 36, "asset": asset }], "level.0": 0.25 }),
            "play",
            36.0,
        ),
        (
            "sample_instrument",
            json!({ "zones": [{ "root_note": 60, "asset": asset }], "level.0": 0.25 }),
            "gate",
            1.0,
        ),
    ];
    for (type_id, config, port, value) in cases {
        let built = registry.build(type_id, SAMPLE_RATE, &config).unwrap();
        let crate::factory::GraphModule::Module(mut module) = built.module else {
            unreachable!("a sampler is not a sink")
        };
        if type_id == "sample_instrument" {
            module.set_input("frequency", 261.63).unwrap();
        }
        module.set_input(port, value).unwrap();
        module.process(64);
        assert!((module.get_output("audio_left").unwrap() - 0.125).abs() < 1e-3);
        let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
            for _ in 0..32 {
                module.process(64);
            }
        });
        assert_eq!((allocs, frees), (0, 0), "{type_id}");
    }
}
