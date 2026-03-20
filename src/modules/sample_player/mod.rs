//! Sample player module for audio file playback.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
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
        let controls = SamplePlayerControls::new(sample_rate, source)?;
        let player = SamplePlayer::new_with_controls(controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(player)),
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
    frame_index: usize,
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
            frame_index: 0,
            playing: false,
            last_play_input: 0.0,
            last_play_trigger: 0,
            last_control_play: false,
            pending_start_gate: false,
            last_processed_sample: 0,
        }
    }

    fn restart(&mut self) {
        self.frame_index = 0;
        self.playing = self.sample.as_ref().map(|sample| sample.len() > 0).unwrap_or(false);
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
            self.frame_index = 0;
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
            self.frame_index = 0;
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
            if self.playing && sample.len() > 0 {
                let index = self.frame_index.min(sample.len() - 1);
                left = sample.left[index];
                right = sample.right[index];

                if index + 1 >= sample.len() {
                    end_gate = 1.0;
                    if loop_enabled {
                        self.frame_index = 0;
                        self.pending_start_gate = true;
                    } else {
                        self.playing = false;
                        self.frame_index = 0;
                        if control_play {
                            self.ctrl.set_play(false);
                            self.last_control_play = false;
                        }
                    }
                } else {
                    self.frame_index += 1;
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

    fn temp_wav_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, nanos))
    }

    fn write_test_wav(
        sample_rate: u32,
        channels: u16,
        frames: &[[f32; 2]],
    ) -> PathBuf {
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
        let path = write_test_wav(
            44_100,
            2,
            &[[0.25, -0.25], [0.5, -0.5], [0.75, -0.75]],
        );

        let controls = SamplePlayerControls::new(44_100, Some(path.to_str().unwrap())).unwrap();
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
        let controls = SamplePlayerControls::new(44_100, Some(path.to_str().unwrap())).unwrap();
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
        let controls = SamplePlayerControls::new(44_100, Some(path.to_str().unwrap())).unwrap();
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
        let path = write_test_wav(
            22_050,
            1,
            &[[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.5, 0.0]],
        );
        let controls = SamplePlayerControls::new(44_100, Some(path.to_str().unwrap())).unwrap();
        let sample_len = controls
            .shared
            .lock()
            .unwrap()
            .pending_sample
            .as_ref()
            .unwrap()
            .len();
        assert!(sample_len >= 7, "expected resampled buffer, got {}", sample_len);

        let _ = std::fs::remove_file(path);
    }
}
