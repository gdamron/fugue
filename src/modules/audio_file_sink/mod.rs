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
    /// Zero block returned by `sink_block` when monitoring is disabled.
    silence: [f32; MAX_BLOCK],
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

    fn process(&mut self, frames: usize) -> bool {
        let stopping = self.shared.is_stopping();
        for i in 0..frames {
            let (mut left, mut right) = (self.inputs.audio_left(i), self.inputs.audio_right(i));
            if self.soft_clip {
                left = Self::soft_clip_sample(left);
                right = Self::soft_clip_sample(right);
            }
            self.outputs.set(i, left, right);

            if !stopping {
                self.shared.push(left, right);
            }
        }
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
    fn factory_exposes_ports_without_building() {
        let factory = AudioFileSinkFactory;
        assert!(factory.is_sink());
        assert_eq!(factory.input_ports(), Some(&inputs::INPUTS[..]));
        assert_eq!(factory.output_ports(), Some(&outputs::OUTPUTS[..]));
    }
}
