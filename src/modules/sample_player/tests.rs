use super::*;
use crate::control_request::apply_declared;
use crate::{ControlSurface, ControlValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

mod audio_thread;

/// Process-unique suffix so parallel tests never collide on a temp path.
/// (The resample cache is keyed by path, so a reused path would otherwise
/// serve a stale buffer.)
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

fn temp_wav_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, unique_suffix()))
}

#[cfg(not(target_arch = "wasm32"))]
fn temp_flac_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fugue-{}-{}.flac", name, unique_suffix()))
}

/// Encodes frames to a 16-bit FLAC bitstream for decode tests.
#[cfg(not(target_arch = "wasm32"))]
fn encode_test_flac(sample_rate: u32, channels: usize, frames: &[[f32; 2]]) -> Vec<u8> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let scale = i16::MAX as f32;
    let mut samples = Vec::new();
    for frame in frames {
        samples.push((frame[0].clamp(-1.0, 1.0) * scale).round() as i32);
        if channels > 1 {
            samples.push((frame[1].clamp(-1.0, 1.0) * scale).round() as i32);
        }
    }

    let config = flacenc::config::Encoder::default().into_verified().unwrap();
    let source =
        flacenc::source::MemSource::from_samples(&samples, channels, 16, sample_rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).unwrap();
    sink.as_slice().to_vec()
}

fn write_test_wav(sample_rate: u32, channels: u16, frames: &[[f32; 2]]) -> PathBuf {
    let path = temp_wav_path("sample-player");
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for frame in frames {
        writer
            .write_sample((frame[0].clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
        if channels > 1 {
            writer
                .write_sample((frame[1].clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .unwrap();
        }
    }
    writer.finalize().unwrap();
    path
}

/// A player built from `config` at 44.1 kHz, and its surface.
fn built(config: serde_json::Value) -> (SamplePlayer, DeclaredSurface) {
    build(44_100, &config).unwrap()
}

/// A classic player holding the sample at `path`, stopped.
fn player(path: &std::path::Path) -> (SamplePlayer, DeclaredSurface) {
    built(serde_json::json!({ "asset": path.to_str().unwrap() }))
}

/// Applies a write as the thread running the player does, between blocks.
pub(super) fn write(player: &mut SamplePlayer, key: &str, value: impl Into<ControlValue>) {
    let index = player.table.resolve(key).unwrap();
    let value = player.table.coerce(index, &value.into()).unwrap();
    apply_declared(player, index, value).unwrap();
}

/// What control `key` holds, as clients read it.
pub(super) fn read(player: &SamplePlayer, key: &str) -> ControlValue {
    let index = player.table.resolve(key).unwrap();
    let value = player.cells.load(index).unwrap();
    player.table.value(index, value).unwrap()
}

/// Loads `asset` as a write to `asset` does, on this thread, and hands it
/// to the player between blocks.
pub(super) fn swap(player: &mut SamplePlayer, surface: &DeclaredSurface, asset: &str) {
    let (payload, _) = surface
        .prepare_payload("asset", &asset.into())
        .unwrap()
        .unwrap();
    let replaced = player.apply_payload(ASSET, payload).ok().unwrap();
    drop(replaced);
}

#[test]
fn test_sample_player_load_and_playback() {
    let path = write_test_wav(44_100, 2, &[[0.25, -0.25], [0.5, -0.5], [0.75, -0.75]]);
    let (mut player, _) = player(&path);
    assert_eq!(read(&player, "playing"), false.into());

    write(&mut player, "play", true);
    assert_eq!(read(&player, "playing"), true.into());
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);
    assert!(player.get_output("audio_left").unwrap() > 0.2);
    assert!(player.get_output("audio_right").unwrap() < -0.2);

    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 0.0);

    player.process(1);
    assert_eq!(player.get_output("end").unwrap(), 1.0);
    assert_eq!(read(&player, "playing"), false.into());
    assert_eq!(read(&player, "play"), true.into(), "only requests write it");

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_loop_input_overrides_control() {
    let path = write_test_wav(44_100, 1, &[[0.1, 0.0], [0.2, 0.0]]);
    let (mut player, _) = player(&path);

    write(&mut player, "play", true);
    player.set_input("loop", 1.0).unwrap();

    player.process(1);
    player.process(1);
    assert_eq!(player.get_output("end").unwrap(), 1.0);
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);
    assert_eq!(read(&player, "playing"), true.into());

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_failed_asset_write_keeps_previous_sample() {
    let path = write_test_wav(44_100, 1, &[[0.3, 0.0]]);
    let (mut player, surface) = player(&path);
    let missing = ControlValue::from("/definitely/missing.wav");

    // Checked by name only, so an edit batch commits and the write
    // reports the failed load.
    let surfaces = Default::default();
    assert!(surface
        .validate_control("asset", &missing, &surfaces)
        .is_ok());
    let refused = surface.prepare_payload("asset", &missing).unwrap();
    assert!(refused.unwrap_err().contains("missing.wav"));
    assert!(surface.set_control("asset", missing).is_err());
    assert_eq!(
        surface.get_control("asset").unwrap(),
        path.to_string_lossy().as_ref().into()
    );

    write(&mut player, "play", true);
    player.process(1);
    assert!(player.get_output("audio_left").unwrap() > 0.25);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_resamples_on_load() {
    let path = write_test_wav(22_050, 1, &[[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.5, 0.0]]);
    let (player, _) = player(&path);
    let sample_len = player.sample.len();
    assert!(
        sample_len >= 7,
        "expected resampled buffer, got {}",
        sample_len
    );

    let _ = std::fs::remove_file(path);
}

/// Counts how many `process()` calls it takes to reach the end-of-sample
/// gate at a given pitch ratio.
fn frames_to_end(path: &std::path::Path, pitch_ratio: f32) -> usize {
    let (mut player, _) = built(serde_json::json!({
        "asset": path.to_str().unwrap(),
        "autoplay": true,
        "pitch_ratio": pitch_ratio,
    }));

    for count in 1..1000 {
        player.process(1);
        if player.get_output("end").unwrap() > 0.5 {
            return count;
        }
    }
    panic!("sample never reached end gate at pitch {}", pitch_ratio);
}

#[test]
fn test_sample_player_pitch_ratio_advances_faster() {
    let frames: Vec<[f32; 2]> = (0..64).map(|i| [i as f32 / 128.0, 0.0]).collect();
    let path = write_test_wav(44_100, 1, &frames);

    let normal = frames_to_end(&path, 1.0);
    let fast = frames_to_end(&path, 2.0);

    // Double the pitch ratio should consume the buffer in roughly half the
    // process calls.
    assert!(
        (fast as f32 - normal as f32 / 2.0).abs() <= 2.0,
        "expected ~{} frames at 2x, got {} (1x took {})",
        normal / 2,
        fast,
        normal
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_pitch_input_overrides_control() {
    let frames: Vec<[f32; 2]> = (0..64).map(|i| [i as f32 / 128.0, 0.0]).collect();
    let path = write_test_wav(44_100, 1, &frames);

    // Control says 1.0, but a connected CV input says 2.0 and must win.
    let (mut player, _) = built(serde_json::json!({
        "asset": path.to_str().unwrap(),
        "autoplay": true,
        "pitch_ratio": 1.0,
    }));

    let mut count = 0;
    for _ in 0..1000 {
        player.set_input("pitch_ratio", 2.0).unwrap();
        player.process(1);
        count += 1;
        if player.get_output("end").unwrap() > 0.5 {
            break;
        }
    }

    // 64 frames at 2x ≈ 32 process calls, well under the 64 a 1x read needs.
    assert!(
        count <= 40,
        "expected ~32 frames with pitch CV 2x, got {}",
        count
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_caches_resampled_buffer() {
    let path = write_test_wav(22_050, 1, &[[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.5, 0.0]]);
    let (first, _) = player(&path);
    let (second, _) = player(&path);

    // Same source + target rate must hand back the same cached buffer.
    assert!(std::ptr::eq(&*first.sample, &*second.sample));

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_factory_accepts_asset_path_object() {
    let path = write_test_wav(44_100, 1, &[[0.4, 0.0], [0.2, 0.0]]);
    let config = serde_json::json!({ "asset": { "path": path.to_str().unwrap() } });
    let (player, surface) = built(config);
    assert_eq!(
        surface.get_control("asset").unwrap(),
        path.to_string_lossy().as_ref().into()
    );
    assert_eq!(player.sample.len(), 2);

    let _ = std::fs::remove_file(path);
}

#[test]
fn the_retired_config_spellings_are_refused() {
    let registry = crate::ModuleRegistry::default();
    for key in ["source", "loop_enabled"] {
        let config = serde_json::json!({ key: true });
        let error = registry
            .build("sample_player", 44_100, &config)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(key), "{key}: {error}");
    }
}

#[test]
fn the_interface_is_named_by_the_convention() {
    let path = write_test_wav(44_100, 1, &[[0.5, 0.5], [0.25, 0.25]]);
    let config = serde_json::json!({
        "asset": path.to_str().unwrap(),
        "autoplay": true,
        "loop": true,
        "pitch_ratio": 2.0,
    });
    let mut built = SamplePlayerFactory.build(44_100, &config).unwrap();
    let surface = built.control_surface.clone().unwrap();
    assert_eq!(
        surface.get_control("asset").unwrap(),
        path.to_string_lossy().as_ref().into()
    );
    assert_eq!(surface.get_control("play").unwrap(), true.into());
    assert_eq!(surface.get_control("playing").unwrap(), true.into());
    assert_eq!(surface.get_control("loop").unwrap(), true.into());
    assert_eq!(surface.get_control("pitch_ratio").unwrap(), 2.0f32.into());
    assert!(surface.get_control("source").is_err());
    let keys: Vec<_> = surface
        .controls()
        .into_iter()
        .map(|meta| meta.key)
        .collect();
    assert_eq!(keys, ["asset", "play", "playing", "loop", "pitch_ratio"]);

    let player = built.module.module_mut();
    assert_eq!(player.inputs(), ["play", "loop", "pitch_ratio"]);
    assert_eq!(
        player.outputs(),
        ["audio_left", "audio_right", "start", "end"]
    );
    player.set_input("pitch_ratio", 1.0).unwrap();
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_documents_play_starts_playback_on_load_as_autoplay_does() {
    let path = write_test_wav(44_100, 1, &[[0.5, 0.0]; 4]);
    let asset = path.to_str().unwrap();
    for (config, plays) in [
        (serde_json::json!({ "asset": asset, "play": true }), true),
        (
            serde_json::json!({ "asset": asset, "autoplay": true }),
            true,
        ),
        (
            serde_json::json!({ "asset": asset, "autoplay": true, "play": false }),
            false,
        ),
        (serde_json::json!({ "asset": asset, "play": "true" }), true),
        (serde_json::json!({ "asset": asset }), false),
    ] {
        let (mut player, surface) = built(config.clone());
        player.process(1);
        let started = player.get_output("start").unwrap() == 1.0;
        assert_eq!(started, plays, "{config}");
        assert_eq!(surface.get_control("play").unwrap(), plays.into());
    }
    let refused = build(44_100, &serde_json::json!({ "play": 1 }))
        .err()
        .unwrap();
    assert!(refused.to_string().contains("'play'"), "{refused}");
    let _ = std::fs::remove_file(path);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_asset_write_resolves_package_ref() {
    // A package ref set live (set_control / add_module path) resolves through
    // the cache dir; the authored ref stays the control value.
    let tmp = tempfile::tempdir().unwrap();
    let pack_dir = tmp.path().join("fugue.test.kit").join("1.0.0");
    std::fs::create_dir_all(&pack_dir).unwrap();
    let wav = write_test_wav(44_100, 1, &[[0.5, 0.0], [0.25, 0.0]]);
    std::fs::rename(&wav, pack_dir.join("kick.wav")).unwrap();

    // Route the default cache lookup at the temp dir for this call only.
    let (mut player, surface) = built(serde_json::json!({}));
    assert_eq!(surface.get_control("asset").unwrap(), "".into());
    crate::pkg::audio_asset::with_packs_dir(tmp.path(), || {
        swap(&mut player, &surface, "fugue.test.kit@1.0.0:kick.wav");
    });
    assert_eq!(player.sample.len(), 2);
}

/// A stereo ramp at least one FLAC block (16 frames) long.
#[cfg(not(target_arch = "wasm32"))]
fn flac_test_frames() -> Vec<[f32; 2]> {
    (0..32)
        .map(|index| [index as f32 / 64.0, -(index as f32) / 64.0])
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_sample_player_loads_flac() {
    let frames = flac_test_frames();
    let path = temp_flac_path("load");
    std::fs::write(&path, encode_test_flac(44_100, 2, &frames)).unwrap();

    let (player, _) = player(&path);
    let sample = &player.sample;
    assert_eq!(sample.len(), frames.len());
    for (index, frame) in frames.iter().enumerate() {
        assert!((sample.left[index] - frame[0]).abs() < 1e-3);
        assert!((sample.right[index] - frame[1]).abs() < 1e-3);
    }

    let _ = std::fs::remove_file(path);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_sample_player_detects_flac_by_header() {
    // FLAC bitstream behind a misleading `.wav` extension: the header magic
    // must win so the file still decodes.
    let frames = flac_test_frames();
    let path = temp_wav_path("flac-as-wav");
    std::fs::write(&path, encode_test_flac(44_100, 1, &frames)).unwrap();

    let (player, _) = player(&path);
    let sample = &player.sample;
    assert_eq!(sample.len(), frames.len());
    assert!((sample.left[5] - frames[5][0]).abs() < 1e-3);

    let _ = std::fs::remove_file(path);
}

/// Counts process calls to end-gate for an elastic player at given ratios,
/// the time ratio from its config.
fn elastic_frames_to_end(path: &std::path::Path, time_ratio: f32, pitch_ratio: f32) -> usize {
    let (mut player, _) = built(serde_json::json!({
        "asset": path.to_str().unwrap(),
        "mode": "elastic",
        "autoplay": true,
        "time_ratio": time_ratio,
    }));
    write(&mut player, "pitch_ratio", pitch_ratio);

    for count in 1..1000 {
        player.process(1);
        if player.get_output("end").unwrap() > 0.5 {
            return count;
        }
    }
    panic!(
        "sample never reached end gate at time {} pitch {}",
        time_ratio, pitch_ratio
    );
}

#[test]
fn test_elastic_time_ratio_scales_duration_and_pitch_does_not() {
    let frames: Vec<[f32; 2]> = (0..64).map(|i| [i as f32 / 128.0, 0.0]).collect();
    let path = write_test_wav(44_100, 1, &frames);

    // time_ratio governs duration exactly: 64 frames at 1x, 32 at 2x.
    assert_eq!(elastic_frames_to_end(&path, 1.0, 1.0), 64);
    assert_eq!(elastic_frames_to_end(&path, 2.0, 1.0), 32);
    assert_eq!(elastic_frames_to_end(&path, 0.5, 1.0), 128);

    // pitch_ratio must not change the duration in elastic mode.
    assert_eq!(elastic_frames_to_end(&path, 1.0, 2.0), 64);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_elastic_mode_gates_time_ratio_control() {
    let (_, classic) = built(serde_json::json!({}));
    assert!(classic
        .set_control("time_ratio", ControlValue::Number(2.0))
        .is_err());
    assert!(!classic
        .controls()
        .iter()
        .any(|meta| meta.key == "time_ratio"));

    let (_, elastic) = built(serde_json::json!({ "mode": "elastic" }));
    elastic
        .set_control("time_ratio", ControlValue::Number(2.0))
        .unwrap();
    assert_eq!(elastic.get_control("time_ratio").unwrap(), 2.0f32.into());
    assert!(elastic
        .controls()
        .iter()
        .any(|meta| meta.key == "time_ratio"));
}

#[test]
fn test_factory_parses_elastic_mode() {
    let bad = serde_json::json!({ "mode": "granular" });
    let err = SamplePlayerFactory
        .build(44_100, &bad)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("'mode'"), "{err}");
}

#[test]
fn test_elastic_asset_write_precomputes_analysis() {
    let path = write_test_wav(44_100, 1, &[[0.5, 0.0], [0.25, 0.0]]);
    let (built, _) =
        built(serde_json::json!({ "asset": path.to_str().unwrap(), "mode": "elastic" }));
    assert!(built.sample.cached_elastic_analysis().is_some(), "at build");

    let other = write_test_wav(44_100, 1, &[[0.5, 0.0]; 3]);
    let (mut player, surface) = built_elastic();
    swap(&mut player, &surface, other.to_str().unwrap());
    assert!(
        player.sample.cached_elastic_analysis().is_some(),
        "an elastic asset write must leave the analysis ready for the audio thread"
    );

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(other);
}

fn built_elastic() -> (SamplePlayer, DeclaredSurface) {
    built(serde_json::json!({ "mode": "elastic" }))
}
