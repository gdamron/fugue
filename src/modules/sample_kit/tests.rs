use super::*;
use crate::factory::ModuleFactory;
use crate::{ControlSurface, ControlValue};
use std::path::PathBuf;
use std::sync::Arc;
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

fn build_kit(
    config: serde_json::Value,
) -> (
    Box<dyn crate::Module + Send>,
    Arc<dyn ControlSurface + Send + Sync>,
) {
    let result = SampleKitFactory.build(44_100, &config).unwrap();
    let surface = result.control_surface.unwrap();
    let module = match result.module {
        crate::factory::GraphModule::Module(module) => module,
        crate::factory::GraphModule::Sink(_) => panic!("sample_kit is not a sink"),
    };
    (module, surface)
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
    let (mut kit, _surface) = build_kit(two_slot_config(&kick, &snare));

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
    let (mut kit, _surface) = build_kit(two_slot_config(&kick, &snare));

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
    let (mut kit, _surface) = build_kit(two_slot_config(&kick, &snare));

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
    let (mut kit, _surface) = build_kit(two_slot_config(&kick, &snare));

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
    let (mut kit, _surface) = build_kit(config);

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
        let (mut kit, surface) = build_kit(config);
        // The authored ref stays the control value.
        assert_eq!(
            surface.get_control("asset.0").unwrap(),
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
    assert_eq!(keys, ["play", "asset.0", "level.0", "key.0"]);

    let (mut kit, _) = build_kit(serde_json::Value::Null);
    assert!(kit.set_input("trigger", 1.0).is_err());
    assert_eq!(kit.inputs(), ["play", "key"]);
    let _ = std::fs::remove_file(kick);
}

#[test]
fn a_recorded_level_wins_over_the_authored_one_and_a_recorded_key_is_refused() {
    let kick = write_level_wav(0.5, 4);
    let asset = kick.to_str().unwrap();
    let slot = serde_json::json!([{ "key": 36, "asset": asset, "level": 0.5 }]);
    let (mut kit, surface) = build_kit(serde_json::json!({ "samples": slot, "level.0": 0.25 }));
    assert_eq!(
        surface.get_control("level.0"),
        Ok(ControlValue::Number(0.25))
    );
    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.125).abs() < TOL);

    let error = SampleKitFactory
        .build(
            44_100,
            &serde_json::json!({ "samples": slot, "key.0": "38" }),
        )
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("fixed at build time"), "{error}");
    let _ = std::fs::remove_file(kick);
}

#[test]
fn a_play_while_building_is_refused_as_any_event_is() {
    let kick = write_level_wav(0.5, 4);
    let slot = serde_json::json!([{ "key": 36, "asset": kick.to_str().unwrap() }]);
    let (_, surface) = build_kit(serde_json::json!({ "samples": slot }));
    assert!(surface.set_control("play", 36.0.into()).is_err());
    let _ = std::fs::remove_file(kick);
}

#[test]
fn a_sample_swapped_out_while_it_sounds_plays_to_its_end() {
    let kick = write_levels_wav(&[0.5, 0.4, 0.3]);
    let quiet = write_level_wav(0.125, 4);
    let slot = serde_json::json!([{ "key": 36, "asset": kick.to_str().unwrap() }]);
    let (mut kit, _surface) = build_kit(serde_json::json!({ "samples": slot }));
    let quiet = crate::modules::sample_loading::load_cached_sample(quiet.to_str().unwrap(), 44_100);
    let payload = crate::payload::Payload::from_arc(quiet.unwrap());
    let asset = controls::index_of(KitControl::Asset(0), 1);

    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    let retired = kit.apply_payload(asset, payload).ok().unwrap();
    let mut heard = Vec::new();
    for _ in 0..3 {
        kit.process(1);
        heard.push(kit.get_output("audio_left").unwrap());
    }
    drop(retired);
    assert!(
        (heard[0] - 0.4).abs() < TOL && (heard[1] - 0.3).abs() < TOL,
        "{heard:?}"
    );
    assert_eq!(heard[2], 0.0, "it ends, and the new one waits for a play");
    kit.set_input("play", 0.0).unwrap();
    kit.process(1);
    kit.set_input("play", 36.0).unwrap();
    kit.process(1);
    assert!((kit.get_output("audio_left").unwrap() - 0.125).abs() < TOL);
}

#[test]
fn the_modules_own_setter_refuses_play_which_its_surface_reads() {
    let kick = write_level_wav(0.5, 4);
    let slot = serde_json::json!([{ "key": 36, "asset": kick.to_str().unwrap() }]);
    let (mut kit, _surface) = build_kit(serde_json::json!({ "samples": slot }));
    for value in [0.0, 36.0] {
        let error = kit.set_control("play", value).unwrap_err();
        assert!(error.contains("read by its module's own surface"), "{error}");
    }
    kit.process(1);
    assert_eq!(kit.get_output("audio_left").unwrap(), 0.0, "nothing played");
    assert!(kit.set_control("level.0", 0.5).is_ok());
    let _ = std::fs::remove_file(kick);
}
