//! Audio file sink module for recording graph audio.
//!
//! This module holds the cross-platform module plumbing. The actual capture
//! backend is platform-specific and lives in `native` (background writer
//! thread + filesystem, WAV/FLAC) or `wasm` (synchronous in-memory WAV).
//! Both expose the same small interface on their shared state — `activate`,
//! `push`, `is_stopping`, `stop`, `finish`, `frames_written`,
//! `frames_dropped` — so the code here never branches on the target.
//!
//! The native backend creates its file only once the sink first processes
//! audio, so a sink built and dropped without playing (a document or edit
//! being checked) never touches the file.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::modules::dac::{finite_or_silence, soft_clip};
use crate::{Module, SinkModule, MAX_BLOCK};

mod inputs;
mod outputs;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::OutputFormat;
#[cfg(not(target_arch = "wasm32"))]
use native::SharedHandle;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm::SharedHandle;

pub struct AudioFileSinkFactory;

const TYPE_ID: &str = "audio_file_sink";

impl ModuleFactory for AudioFileSinkFactory {
    fn build_for_inspection(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        #[cfg(not(target_arch = "wasm32"))]
        native::inspection_config(config)?;
        #[cfg(target_arch = "wasm32")]
        wasm::max_frames(config, _sample_rate)?;
        Ok(crate::factory::inspection_sink(
            &inputs::INPUTS,
            &outputs::OUTPUTS,
        ))
    }

    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        use crate::module_config::ConfigKey;
        #[cfg(not(target_arch = "wasm32"))]
        return const {
            &[
                native::BUFFER_FRAMES,
                ConfigKey::text("path"),
                ConfigKey::boolean("soft_clip"),
                ConfigKey::boolean("monitor"),
            ]
        };
        #[cfg(target_arch = "wasm32")]
        return const {
            &[
                wasm::MAX_FRAMES,
                wasm::MAX_DURATION,
                ConfigKey::boolean("soft_clip"),
                ConfigKey::boolean("monitor"),
            ]
        };
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        #[cfg(not(target_arch = "wasm32"))]
        let (sink, handle) = native::build(config, sample_rate)?;
        #[cfg(target_arch = "wasm32")]
        let (sink, handle) = wasm::build(config, sample_rate)?;

        Ok(ModuleBuildResult {
            module: GraphModule::Sink(Box::new(sink)),
            handles: vec![(
                "handle".to_string(),
                Arc::new(handle) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: None,
            sink: Some(()),
        })
    }

    fn is_sink(&self) -> bool {
        true
    }

    fn input_ports(&self) -> Option<&'static [&'static str]> {
        Some(&inputs::INPUTS)
    }

    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&outputs::OUTPUTS)
    }
}

pub struct AudioFileSink {
    inputs: inputs::AudioFileSinkInputs,
    outputs: outputs::AudioFileSinkOutputs,
    shared: SharedHandle,
    soft_clip: bool,
    monitor: bool,
    /// Zero block returned by `sink_block` when monitoring is disabled.
    silence: [f32; MAX_BLOCK],
    /// Whether this sink has told its backend it is recording, which it
    /// does once, on the first block it processes.
    activated: bool,
    /// Non-finite samples replaced with silence since the sink was built.
    non_finite_samples: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AudioFileSinkStats {
    pub frames_written: usize,
    pub frames_dropped: usize,
}

#[derive(Clone)]
pub struct AudioFileSinkHandle {
    shared: SharedHandle,
}

impl AudioFileSink {
    /// Builds the module and its host handle from a platform shared state.
    /// Called by the platform backends.
    fn from_shared(
        shared: SharedHandle,
        soft_clip: bool,
        monitor: bool,
    ) -> (Self, AudioFileSinkHandle) {
        let sink = Self {
            inputs: inputs::AudioFileSinkInputs::new(),
            outputs: outputs::AudioFileSinkOutputs::new(),
            shared: shared.clone(),
            soft_clip,
            monitor,
            silence: [0.0; MAX_BLOCK],
            activated: false,
            non_finite_samples: 0,
        };
        let handle = AudioFileSinkHandle { shared };
        (sink, handle)
    }

    pub fn with_monitor(mut self, monitor: bool) -> Self {
        self.monitor = monitor;
        self
    }

    pub fn with_soft_clip(mut self, soft_clip: bool) -> Self {
        self.soft_clip = soft_clip;
        self
    }
}

impl Drop for AudioFileSink {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl Module for AudioFileSink {
    fn name(&self) -> &str {
        "AudioFileSink"
    }

    fn process(&mut self, frames: usize) -> bool {
        if !self.activated {
            // The destination is opened only once audio reaches the sink, so
            // one built and dropped unplayed (a check, or an edit refused
            // after it was prepared) never touches it.
            self.activated = true;
            self.shared.activate();
        }
        let stopping = self.shared.is_stopping();
        let mut replaced = 0;
        for i in 0..frames {
            let mut left = finite_or_silence(self.inputs.audio_left(i), &mut replaced);
            let mut right = finite_or_silence(self.inputs.audio_right(i), &mut replaced);
            if self.soft_clip {
                left = soft_clip(left);
                right = soft_clip(right);
            }
            self.outputs.set(i, left, right);

            if !stopping {
                self.shared.push(left, right);
            }
        }
        self.non_finite_samples += replaced;
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }
}

impl SinkModule for AudioFileSink {
    fn sink_block(&self) -> (&[f32], &[f32]) {
        if self.monitor {
            (self.outputs.left_block(), self.outputs.right_block())
        } else {
            (&self.silence, &self.silence)
        }
    }

    fn non_finite_samples(&self) -> u64 {
        self.non_finite_samples
    }
}

impl AudioFileSinkHandle {
    /// Stops capture, drains/finalizes the backend, and returns final stats.
    pub fn finish(&self) -> AudioFileSinkStats {
        self.shared.finish();
        self.stats()
    }

    /// Why the recording failed, if it did: its file could not be created
    /// when audio first reached the sink, or could not be written.
    pub fn error(&self) -> Option<String> {
        self.shared.error()
    }

    pub fn stats(&self) -> AudioFileSinkStats {
        AudioFileSinkStats {
            frames_written: self.shared.frames_written(),
            frames_dropped: self.shared.frames_dropped(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use std::path::PathBuf;
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(not(target_arch = "wasm32"))]
    fn temp_wav_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, nanos))
    }

    #[test]
    fn monitor_controls_sink_output() {
        #[cfg(not(target_arch = "wasm32"))]
        let path = temp_wav_path("audio-file-sink-monitor");
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), OutputFormat::Wav, 44_100, false, false, 16).unwrap();
        #[cfg(target_arch = "wasm32")]
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, false, false, 16).unwrap();

        sink.set_input("audio_left", 0.2).unwrap();
        sink.set_input("audio_right", 0.4).unwrap();
        sink.process(1);
        // Monitoring disabled: sink contributes silence to the mix.
        let (left, right) = sink.sink_block();
        assert_eq!(left[0], 0.0);
        assert_eq!(right[0], 0.0);

        sink = sink.with_monitor(true);
        let (left, right) = sink.sink_block();
        assert_eq!(left[0], 0.2);
        assert_eq!(right[0], 0.4);

        handle.finish();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn soft_clip_can_be_disabled() {
        #[cfg(not(target_arch = "wasm32"))]
        let path = temp_wav_path("audio-file-sink-clip");
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), OutputFormat::Wav, 44_100, true, true, 16).unwrap();
        #[cfg(target_arch = "wasm32")]
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, true, true, 16).unwrap();

        sink.set_input("audio", 3.0).unwrap();
        sink.process(1);
        assert!(sink.sink_block().0[0] < 1.0);

        sink = sink.with_soft_clip(false);
        sink.set_input("audio", 3.0).unwrap();
        sink.process(1);
        let (left, right) = sink.sink_block();
        assert_eq!(left[0], 3.0);
        assert_eq!(right[0], 3.0);

        handle.finish();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn non_finite_samples_are_recorded_and_monitored_as_silence() {
        #[cfg(not(target_arch = "wasm32"))]
        let path = temp_wav_path("audio-file-sink-non-finite");
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), OutputFormat::Wav, 44_100, true, true, 16).unwrap();
        #[cfg(target_arch = "wasm32")]
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, true, true, 16).unwrap();

        sink.set_input("audio_left", f32::NAN).unwrap();
        sink.set_input("audio_right", 0.5).unwrap();
        sink.process(2);
        let (left, right) = sink.sink_block();
        assert_eq!(
            (&left[..2], &right[..2]),
            (&[0.0, 0.0][..], &[0.5, 0.5][..])
        );
        assert_eq!(sink.non_finite_samples(), 2);

        handle.finish();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn factory_exposes_ports_without_building() {
        let factory = AudioFileSinkFactory;
        assert!(factory.is_sink());
        assert_eq!(factory.input_ports(), Some(&inputs::INPUTS[..]));
        assert_eq!(factory.output_ports(), Some(&outputs::OUTPUTS[..]));
    }
}
