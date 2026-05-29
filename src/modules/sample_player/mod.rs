//! Sample player module for audio file playback.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::Module;

pub use self::controls::SamplePlayerControls;

mod controls;
mod inputs;
mod outputs;

pub struct SamplePlayerFactory;

impl ModuleFactory for SamplePlayerFactory {
    fn type_id(&self) -> &'static str {
        "sample_player"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let source = config.get("source").and_then(|value| value.as_str());
        let play = config.get("play").and_then(|value| value.as_bool());
        let loop_enabled = config.get("loop_enabled").and_then(|value| value.as_bool());
        let controls = SamplePlayerControls::new(sample_rate, source, play, loop_enabled)?;
        let player = SamplePlayer::new_with_controls(controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(player)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

pub struct SamplePlayer {
    ctrl: SamplePlayerControls,
    inputs: inputs::SamplePlayerInputs,
    outputs: outputs::SamplePlayerOutputs,
    sample: Option<Arc<controls::SampleData>>,
    position: f64,
    playing: bool,
    last_play_input: f32,
    last_play_trigger: u64,
    last_control_play: bool,
    pending_start_gate: bool,
    last_processed_sample: u64,
}

impl SamplePlayer {
    pub fn new_with_controls(controls: SamplePlayerControls) -> Self {
        Self {
            ctrl: controls,
            inputs: inputs::SamplePlayerInputs::new(),
            outputs: outputs::SamplePlayerOutputs::new(),
            sample: None,
            position: 0.0,
            playing: false,
            last_play_input: 0.0,
            last_play_trigger: 0,
            last_control_play: false,
            pending_start_gate: false,
            last_processed_sample: 0,
        }
    }

    fn restart(&mut self) {
        self.position = 0.0;
        self.playing = self
            .sample
            .as_ref()
            .map(|sample| sample.len() > 0)
            .unwrap_or(false);
        self.pending_start_gate = self.playing;
    }
}

impl Module for SamplePlayer {
    fn name(&self) -> &str {
        "SamplePlayer"
    }

    fn process(&mut self) -> bool {
        let (control_play, play_trigger, loop_control, pending_sample) = {
            let mut shared = self.ctrl.shared.lock().unwrap();
            (
                shared.play,
                shared.play_trigger,
                shared.loop_enabled,
                shared.pending_sample.take(),
            )
        };

        if let Some(sample) = pending_sample {
            self.sample = Some(sample);
            self.position = 0.0;
            self.playing = control_play;
            self.pending_start_gate = self.playing;
        }

        let gate_rising = self.inputs.play() > 0.5 && self.last_play_input <= 0.5;
        if play_trigger != self.last_play_trigger {
            self.last_play_trigger = play_trigger;
            self.restart();
        } else if gate_rising {
            self.restart();
        } else if !control_play && self.last_control_play {
            self.playing = false;
            self.position = 0.0;
            self.pending_start_gate = false;
        }

        self.last_control_play = control_play;

        let loop_enabled = self.inputs.loop_enabled(loop_control);
        let mut start_gate = 0.0;
        let mut end_gate = 0.0;
        let mut left = 0.0;
        let mut right = 0.0;

        if self.pending_start_gate && self.playing {
            start_gate = 1.0;
            self.pending_start_gate = false;
        }

        if let Some(sample) = &self.sample {
            let len = sample.len();
            if self.playing && len > 0 {
                let (l, r) = sample.sample_at(self.position);
                left = l;
                right = r;

                let pitch = self.inputs.pitch(self.ctrl.pitch_ratio()).max(1e-4);
                self.position += pitch as f64;

                if self.position >= len as f64 {
                    end_gate = 1.0;
                    if loop_enabled {
                        // `%=` keeps the fractional read head bounded without
                        // accumulating drift across loops.
                        self.position %= len as f64;
                        self.pending_start_gate = true;
                    } else {
                        self.playing = false;
                        self.position = 0.0;
                        if control_play {
                            self.ctrl.set_play(false);
                            self.last_control_play = false;
                        }
                    }
                }
            }
        }

        self.outputs.set(left, right, start_gate, end_gate);
        self.last_play_input = self.inputs.play();
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }
}

#[cfg(test)]
mod tests {
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
        player.process();
        assert_eq!(player.get_output("sample_start_gate").unwrap(), 1.0);
        assert!(player.get_output("audio_left").unwrap() > 0.2);
        assert!(player.get_output("audio_right").unwrap() < -0.2);

        player.process();
        assert_eq!(player.get_output("sample_start_gate").unwrap(), 0.0);

        player.process();
        assert_eq!(player.get_output("sample_end_gate").unwrap(), 1.0);
        assert_eq!(controls.play(), false);

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

        player.process();
        player.process();
        assert_eq!(player.get_output("sample_end_gate").unwrap(), 1.0);
        player.process();
        assert_eq!(player.get_output("sample_start_gate").unwrap(), 1.0);
        assert_eq!(controls.play(), true);

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
        player.process();
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
            player.process();
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
            player.process();
            player.reset_inputs();
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
}
