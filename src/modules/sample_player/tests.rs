use super::*;
use crate::{ControlSurface, ControlValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let stream =
        flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
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

#[test]
fn test_sample_player_load_and_playback() {
    let path = write_test_wav(44_100, 2, &[[0.25, -0.25], [0.5, -0.5], [0.75, -0.75]]);

    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(false),
        Some(false),
    )
    .unwrap();
    let mut player = SamplePlayer::new_with_controls(controls.clone());

    controls
        .set_control("play", ControlValue::Bool(true))
        .unwrap();
    player.process(1);
    assert_eq!(player.get_output("sample_start_gate").unwrap(), 1.0);
    assert!(player.get_output("audio_left").unwrap() > 0.2);
    assert!(player.get_output("audio_right").unwrap() < -0.2);

    player.process(1);
    assert_eq!(player.get_output("sample_start_gate").unwrap(), 0.0);

    player.process(1);
    assert_eq!(player.get_output("sample_end_gate").unwrap(), 1.0);
    assert!(!controls.play());

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_loop_input_overrides_control() {
    let path = write_test_wav(44_100, 1, &[[0.1, 0.0], [0.2, 0.0]]);
    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(false),
        Some(false),
    )
    .unwrap();
    let mut player = SamplePlayer::new_with_controls(controls.clone());

    controls.set_play(true);
    player.set_input("loop", 1.0).unwrap();

    player.process(1);
    player.process(1);
    assert_eq!(player.get_output("sample_end_gate").unwrap(), 1.0);
    player.process(1);
    assert_eq!(player.get_output("sample_start_gate").unwrap(), 1.0);
    assert!(controls.play());

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_failed_reload_keeps_previous_sample() {
    let path = write_test_wav(44_100, 1, &[[0.3, 0.0]]);
    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(false),
        Some(false),
    )
    .unwrap();
    let mut player = SamplePlayer::new_with_controls(controls.clone());

    let bad = controls.set_source("/definitely/missing.wav");
    assert!(bad.is_err());
    assert_eq!(controls.source(), path.to_string_lossy());

    controls.set_play(true);
    player.process(1);
    assert!(player.get_output("audio_left").unwrap() > 0.25);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_resamples_on_load() {
    let path = write_test_wav(22_050, 1, &[[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.5, 0.0]]);
    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(false),
        Some(false),
    )
    .unwrap();
    let sample_len = controls
        .shared
        .lock()
        .unwrap()
        .pending_sample
        .as_ref()
        .unwrap()
        .len();
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
    let controls =
        SamplePlayerControls::new(44_100, Some(path.to_str().unwrap()), Some(true), Some(false))
            .unwrap();
    controls
        .set_control("pitch_ratio", ControlValue::Number(pitch_ratio))
        .unwrap();
    let mut player = SamplePlayer::new_with_controls(controls);

    for count in 1..1000 {
        player.process(1);
        if player.get_output("sample_end_gate").unwrap() > 0.5 {
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

    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(true),
        Some(false),
    )
    .unwrap();
    // Control says 1.0, but a connected CV input says 2.0 and must win.
    controls
        .set_control("pitch_ratio", ControlValue::Number(1.0))
        .unwrap();
    let mut player = SamplePlayer::new_with_controls(controls);

    let mut count = 0;
    for _ in 0..1000 {
        player.set_input("pitch", 2.0).unwrap();
        player.process(1);
        count += 1;
        if player.get_output("sample_end_gate").unwrap() > 0.5 {
            break;
        }
    }

    // 64 frames at 2x ≈ 32 process calls, well under the 64 a 1x read needs.
    assert!(count <= 40, "expected ~32 frames with pitch CV 2x, got {}", count);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sample_player_caches_resampled_buffer() {
    let path = write_test_wav(22_050, 1, &[[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.5, 0.0]]);
    let source = path.to_str().unwrap();

    let first =
        SamplePlayerControls::new(44_100, Some(source), Some(false), Some(false)).unwrap();
    let second =
        SamplePlayerControls::new(44_100, Some(source), Some(false), Some(false)).unwrap();

    let first_buf = first.shared.lock().unwrap().pending_sample.clone().unwrap();
    let second_buf = second.shared.lock().unwrap().pending_sample.clone().unwrap();

    // Same source + target rate must hand back the same cached Arc.
    assert!(Arc::ptr_eq(&first_buf, &second_buf));

    let _ = std::fs::remove_file(path);
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

    let controls =
        SamplePlayerControls::new(44_100, Some(path.to_str().unwrap()), Some(false), Some(false))
            .unwrap();

    let shared = controls.shared.lock().unwrap();
    let sample = shared.pending_sample.as_ref().unwrap();
    assert_eq!(sample.len(), frames.len());
    for (index, frame) in frames.iter().enumerate() {
        assert!((sample.left[index] - frame[0]).abs() < 1e-3);
        assert!((sample.right[index] - frame[1]).abs() < 1e-3);
    }
    drop(shared);

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

    let controls =
        SamplePlayerControls::new(44_100, Some(path.to_str().unwrap()), Some(false), Some(false))
            .unwrap();

    let shared = controls.shared.lock().unwrap();
    let sample = shared.pending_sample.as_ref().unwrap();
    assert_eq!(sample.len(), frames.len());
    assert!((sample.left[5] - frames[5][0]).abs() < 1e-3);
    drop(shared);

    let _ = std::fs::remove_file(path);
}
