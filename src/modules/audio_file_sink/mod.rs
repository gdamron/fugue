//! Audio file sink module for recording graph audio.
//!
//! This module holds the cross-platform module plumbing. The actual capture
//! backend is platform-specific and lives in [`native`] (background writer
//! thread + filesystem, WAV/FLAC) or [`wasm`] (synchronous in-memory WAV).
//! Both expose the same small interface on their shared state — `push`,
//! `is_stopping`, `stop`, `finish`, `frames_written`, `frames_dropped` — so the
//! code here never branches on the target.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, SinkModule, SinkOutput};

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

impl ModuleFactory for AudioFileSinkFactory {
    fn type_id(&self) -> &'static str {
        "audio_file_sink"
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
    last_processed_sample: u64,
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
            last_processed_sample: 0,
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

    #[inline]
    fn soft_clip_sample(sample: f32) -> f32 {
        const KNEE: f32 = 0.95;
        const HEADROOM: f32 = 1.0 - KNEE;

        let abs = sample.abs();
        if abs <= KNEE {
            sample
        } else {
            let excess = (abs - KNEE) / HEADROOM;
            let compressed = KNEE + HEADROOM * excess / (1.0 + excess);
            sample.signum() * compressed
        }
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

    fn process(&mut self) -> bool {
        let (mut left, mut right) = (self.inputs.audio_left(), self.inputs.audio_right());
        if self.soft_clip {
            left = Self::soft_clip_sample(left);
            right = Self::soft_clip_sample(right);
        }
        self.outputs.set(left, right);

        if !self.shared.is_stopping() {
            self.shared.push(left, right);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[inline]
    fn set_input_by_index(&mut self, index: usize, value: f32) {
        self.inputs.set_by_index(index, value);
    }

    #[inline]
    fn get_output_by_index(&self, index: usize) -> f32 {
        self.outputs.get_by_index(index)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}

impl SinkModule for AudioFileSink {
    fn sink_output(&self) -> SinkOutput {
        if self.monitor {
            SinkOutput::stereo(self.outputs.audio_left(), self.outputs.audio_right())
        } else {
            SinkOutput::default()
        }
    }
}

impl AudioFileSinkHandle {
    /// Stops capture, drains/finalizes the backend, and returns final stats.
    pub fn finish(&self) -> AudioFileSinkStats {
        self.shared.finish();
        self.stats()
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
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::default());

        sink = sink.with_monitor(true);
        assert_eq!(sink.sink_output(), SinkOutput::stereo(0.2, 0.4));

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
        sink.process();
        assert!(sink.sink_output().left < 1.0);

        sink = sink.with_soft_clip(false);
        sink.reset_inputs();
        sink.set_input("audio", 3.0).unwrap();
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::stereo(3.0, 3.0));

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
