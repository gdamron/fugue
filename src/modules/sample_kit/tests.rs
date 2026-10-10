use super::*;
use crate::factory::ModuleFactory;
use crate::{ControlSurface, ControlValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-unique suffix so parallel tests never collide on a temp path.
/// (The shared sample cache is keyed by path, so a reused path would
/// otherwise serve a stale buffer.)
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, seq)
}

fn temp_wav_path() -> PathBuf {
    std::env::temp_dir().join(format!("fugue-sample-kit-{}.wav", unique_suffix()))
}

/// A mono WAV holding exactly `levels` as its frames.
fn write_levels_wav(levels: &[f32]) -> PathBuf {
    let path = temp_wav_path();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for level in levels {
        writer
            .write_sample((level.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
    path
}

/// A mono WAV whose every frame has amplitude `level`.
fn write_level_wav(level: f32, frames: usize) -> PathBuf {
    write_levels_wav(&vec![level; frames])
}

fn build_kit(config: serde_json::Value) -> (Box<dyn crate::Module + Send>, SampleKitControls) {
    let result = SampleKitFactory.build(44_100, &config).unwrap();
    let controls = result
        .handles
        .iter()
        .find(|(name, _)| name == "controls")
        .unwrap()
        .1
        .downcast_ref::<SampleKitControls>()
        .unwrap()
        .clone();
    let module = match result.module {
        crate::factory::GraphModule::Module(module) => module,
        crate::factory::GraphModule::Sink(_) => panic!("sample_kit is not a sink"),
    };
    (module, controls)
}

fn two_slot_config(kick: &std::path::Path, snare: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "samples": [
            { "key": 36, "asset": kick.to_str().unwrap() },
            { "key": 38, "asset": snare.to_str().unwrap() }
        ]
    })
}

/// 16-bit WAV quantization tolerance.
const TOL: f32 = 2e-3;

#[test]
fn test_play_value_selects_slot() {
    let kick = write_level_wav(0.5, 4);
    let snare = write_level_wav(0.25, 4);
    let (mut kit, _controls) = build_kit(two_slot_config(&kick, &snare));

    // No `key` connection: the `play` input's own value carries the key.
    kit.set_input("play", 36.0).unwrap();
    kit.process(2);
    assert!((kit.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    assert!((kit.get_output("audio_right").unwrap() - 0.5).abs() < TOL);

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(snare);
}

#[test]
fn test_key_input_selects_slot() {
    let kick = write_level_wav(0.5, 4);
    let snare = write_level_wav(0.25, 4);
    let (mut kit, _controls) = build_kit(two_slot_config(&kick, &snare));

    kit.set_input("key", 38.0).unwrap();
    kit.set_input("play", 1.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(snare);
}

#[test]
fn test_unmatched_key_stays_silent() {
    let kick = write_level_wav(0.5, 4);
    let snare = write_level_wav(0.25, 4);
    let (mut kit, _controls) = build_kit(two_slot_config(&kick, &snare));

    kit.set_input("play", 40.0).unwrap();
    kit.process(1);
    assert_eq!(kit.get_output("audio_left").unwrap(), 0.0);

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(snare);
}

#[test]
fn test_slots_overlap_and_voice_ends() {
    let kick = write_level_wav(0.5, 4);
    let snare = write_level_wav(0.25, 2);
    let (mut kit, _controls) = build_kit(two_slot_config(&kick, &snare));

    kit.set_input("key", 36.0).unwrap();
    kit.set_input("play", 1.0).unwrap();
    kit.process(1);
    kit.set_input("play", 0.0).unwrap();
    kit.process(1);

    // Snare joins while the kick is still sounding: the mix is their sum.
    kit.set_input("key", 38.0).unwrap();
    kit.set_input("play", 1.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.75).abs() < TOL);

    // One frame later the 2-frame snare plays its last frame while the
    // 4-frame kick plays its own last frame.
    kit.set_input("play", 0.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.75).abs() < TOL);

    // Both voices are exhausted: silence.
    kit.process(1);
    assert_eq!(kit.get_output("audio_left").unwrap(), 0.0);

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(snare);
}

#[test]
fn test_replay_restarts_slot() {
    // A decaying sample distinguishes a restart from a continuation.
    let path = write_levels_wav(&[0.5, 0.1, 0.1, 0.1]);
    let config = serde_json::json!({
        "samples": [ { "key": 36, "asset": path.to_str().unwrap() } ]
    });
    let (mut kit, _controls) = build_kit(config);

    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    kit.set_input("play", 0.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.1).abs() < TOL);

    // Replay: playback restarts from the first frame (per-slot choke).
    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_play_control_by_name_and_numeric_string() {
    let kick = write_level_wav(0.5, 4);
    let ride = write_level_wav(0.25, 4);
    let config = serde_json::json!({
        "samples": [
            { "key": 36, "asset": kick.to_str().unwrap() },
            { "key": "ride", "asset": ride.to_str().unwrap() }
        ]
    });
    let (mut kit, controls) = build_kit(config);

    controls
        .set_control("play", ControlValue::String("ride".to_string()))
        .unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    controls
        .set_control("play", ControlValue::String("36".to_string()))
        .unwrap();
    kit.process(1);
    // Kick starts while the ride continues.
    assert!((kit.get_output("audio_left").unwrap() - 0.75).abs() < TOL);

    let err = controls
        .set_control("play", ControlValue::String("crash".to_string()))
        .unwrap_err();
    assert!(err.contains("No sample slot named 'crash'"), "{err}");

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(ride);
}

#[test]
fn test_level_control_scales_slot() {
    let kick = write_level_wav(0.5, 4);
    let config = serde_json::json!({
        "samples": [ { "key": 36, "asset": kick.to_str().unwrap(), "level": 0.5 } ]
    });
    let (mut kit, controls) = build_kit(config);

    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    controls
        .set_control("level.0", ControlValue::Number(2.0))
        .unwrap();
    kit.set_input("play", 0.0).unwrap();
    kit.process(1);
    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 1.0).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(kick);
}

#[test]
fn test_asset_control_swaps_slot_sample() {
    let kick = write_level_wav(0.5, 8);
    let replacement = write_level_wav(0.25, 8);
    let config = serde_json::json!({
        "samples": [ { "key": 36, "asset": kick.to_str().unwrap() } ]
    });
    let (mut kit, controls) = build_kit(config);

    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    // The swap lands at the next block; the new sample waits for a play
    // instead of jumping in mid-buffer.
    controls
        .set_control(
            "asset.0",
            ControlValue::String(replacement.to_str().unwrap().to_string()),
        )
        .unwrap();
    assert_eq!(
        controls.get_control("asset.0").unwrap(),
        ControlValue::String(replacement.to_str().unwrap().to_string())
    );
    kit.process(1);
    assert_eq!(kit.get_output("audio_left").unwrap(), 0.0);

    controls
        .set_control("play", ControlValue::Number(36.0))
        .unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    let _ = std::fs::remove_file(kick);
    let _ = std::fs::remove_file(replacement);
}

#[test]
fn test_null_config_builds_empty_kit() {
    // Module type discovery constructs every registered type with a null
    // config, so an empty kit must build and describe itself.
    let result = SampleKitFactory
        .build(44_100, &serde_json::Value::Null)
        .unwrap();
    let surface = result.control_surface.unwrap();
    let metas = surface.controls();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].key, "play");
}

#[test]
fn test_config_validation_errors() {
    let kick = write_level_wav(0.5, 2);
    let kick_str = kick.to_str().unwrap();

    let cases = [
        (
            serde_json::json!({ "samples": [{ "asset": kick_str }] }),
            "missing 'key'",
        ),
        (
            serde_json::json!({ "samples": [{ "key": 36 }] }),
            "missing 'asset'",
        ),
        (
            serde_json::json!({ "samples": [
                { "key": 36, "asset": kick_str },
                { "key": 36, "asset": kick_str }
            ] }),
            "duplicate key '36'",
        ),
        (
            serde_json::json!({ "samples": [{ "key": "36", "asset": kick_str }] }),
            "must be a JSON number",
        ),
        (
            serde_json::json!({ "samples": [{ "key": 36.5, "asset": kick_str }] }),
            "sample_kit config 'samples[0].key' expects a whole number",
        ),
        (
            serde_json::json!({ "samples": [{ "key": "", "asset": kick_str }] }),
            "must not be empty",
        ),
        (
            serde_json::json!({ "samples": [{ "key": 36, "asset": kick_str, "level": "loud" }] }),
            "'samples[0].level' expects a finite number",
        ),
        (serde_json::json!({ "samples": {} }), "must be an array"),
    ];

    for (config, expected) in cases {
        let err = SampleKitFactory
            .build(44_100, &config)
            .err()
            .unwrap_or_else(|| panic!("expected error for {config}"))
            .to_string();
        assert!(err.contains(expected), "{config}: {err}");
    }

    let _ = std::fs::remove_file(kick);
}

#[test]
fn test_missing_sample_file_errors_with_slot_key() {
    let config = serde_json::json!({
        "samples": [ { "key": 36, "asset": "/nonexistent/kick.wav" } ]
    });
    let err = SampleKitFactory
        .build(44_100, &config)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("slot '36'"), "{err}");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_package_ref_slot_resolves_through_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let pack_dir = tmp.path().join("fugue.test.kit").join("1.0.0");
    std::fs::create_dir_all(&pack_dir).unwrap();
    let wav = write_level_wav(0.5, 4);
    std::fs::rename(&wav, pack_dir.join("kick.wav")).unwrap();

    let config = serde_json::json!({
        "samples": [ { "key": 36, "asset": "fugue.test.kit@1.0.0:kick.wav" } ]
    });
    crate::pkg::audio_asset::with_packs_dir(tmp.path(), || {
        let (mut kit, controls) = build_kit(config);
        // The authored ref stays the control value.
        assert_eq!(
            controls.get_control("asset.0").unwrap(),
            ControlValue::String("fugue.test.kit@1.0.0:kick.wav".to_string())
        );
        kit.set_input("play", 36.0).unwrap();
        kit.process(1);
        assert!((kit.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    });
}

#[test]
fn slot_numbers_read_whole_floats_and_are_refused_with_their_path() {
    let kick = write_level_wav(0.5, 8);
    let slot = |key: serde_json::Value, level: serde_json::Value| {
        let asset = kick.to_str().unwrap();
        let config =
            serde_json::json!({ "samples": [{ "key": key, "asset": asset, "level": level }] });
        SampleKitFactory
            .build(44_100, &config)
            .map(|built| built.control_surface.unwrap())
            .map_err(|error| error.to_string())
    };
    let surface = slot(serde_json::json!(36.0), serde_json::json!(0.5)).unwrap();
    assert_eq!(surface.get_control("key.0"), Ok(ControlValue::from("36")));
    assert_eq!(
        slot(serde_json::json!(36), serde_json::json!(1e39)).err(),
        Some("sample_kit config 'samples[0].level' expects a finite number, got 1e39".into())
    );
    let error = slot(serde_json::json!(3e9), serde_json::json!(1))
        .err()
        .unwrap();
    assert!(
        error.contains("'samples[0].key' expects a whole number from -2147483648"),
        "{error}"
    );
    let _ = std::fs::remove_file(kick);
}

#[test]
fn the_retired_spellings_are_refused() {
    let kick = write_level_wav(0.5, 8);
    let asset = kick.to_str().unwrap();
    let registry = crate::ModuleRegistry::default();
    let build = |config: serde_json::Value| {
        registry
            .build("sample_kit", 44_100, &config)
            .map(|built| built.control_surface.unwrap())
            .map_err(|error| error.to_string())
    };

    let error =
        build(serde_json::json!({ "samples": [{ "key": 36, "asset": asset, "gain": 0.5 }] }))
            .err()
            .unwrap();
    assert!(
        error.contains("samples[0] has no field 'gain'; it takes key, asset, level"),
        "{error}"
    );
    let slot = serde_json::json!([{ "key": 36, "asset": asset }]);
    let error = build(serde_json::json!({ "samples": slot, "gain.0": 0.5 }))
        .err()
        .unwrap();
    assert!(error.contains("has no key 'gain.0'"), "{error}");

    let surface = build(serde_json::json!({ "samples": slot })).unwrap();
    assert!(surface.set_control("trigger", 36.0.into()).is_err());
    assert!(surface.set_control("gain.0", 0.5.into()).is_err());
    let keys: Vec<String> = surface
        .controls()
        .into_iter()
        .map(|meta| meta.key)
        .collect();
    assert_eq!(keys, ["play", "key.0", "asset.0", "level.0"]);

    let mut kit = SampleKit::new_with_controls(
        SampleKitControls::new(44_100, Vec::new()).unwrap().0,
        Vec::new(),
    );
    assert!(kit.set_input("trigger", 1.0).is_err());
    assert_eq!(kit.inputs(), ["play", "key"]);
    let _ = std::fs::remove_file(kick);
}
