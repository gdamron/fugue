//! RTMP streaming tap backed by an ffmpeg subprocess.
//!
//! Audio passes through the module unchanged and is also handed off to the
//! shared streaming backend through a lock-free ring. Video is pushed by the host
//! through [`RtmpSinkHandle::push_video_rgba`]. The native backend owns ffmpeg
//! and all socket/process I/O on a worker thread.
//!
//! Requires an `ffmpeg` executable in `PATH` by default. Set
//! `config.ffmpeg_path` to an explicit executable path when the host bundles or
//! discovers ffmpeg itself.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, SinkModule, MAX_BLOCK};

mod inputs;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod outputs;

#[cfg(not(target_arch = "wasm32"))]
use native::SharedHandle;

#[cfg(not(target_arch = "wasm32"))]
pub use native::RtmpSinkConfig;

#[cfg(not(target_arch = "wasm32"))]
pub struct RtmpSinkFactory;

#[cfg(not(target_arch = "wasm32"))]
impl ModuleFactory for RtmpSinkFactory {
    fn type_id(&self) -> &'static str {
        "rtmp_sink"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let (sink, handle) = native::build(config, sample_rate)?;

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

pub struct RtmpSink {
    inputs: inputs::RtmpSinkInputs,
    outputs: outputs::RtmpSinkOutputs,
    shared: SharedHandle,
    soft_clip: bool,
    monitor: bool,
    silence: [f32; MAX_BLOCK],
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RtmpSinkStats {
    pub audio_frames_sent: usize,
    pub audio_frames_dropped: usize,
    pub video_frames_sent: usize,
    pub video_frames_dropped: usize,
    pub restarts: usize,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct RtmpSinkHandle {
    shared: SharedHandle,
}

impl RtmpSink {
    fn from_shared(shared: SharedHandle, soft_clip: bool, monitor: bool) -> (Self, RtmpSinkHandle) {
        let sink = Self {
            inputs: inputs::RtmpSinkInputs::new(),
            outputs: outputs::RtmpSinkOutputs::new(),
            shared: shared.clone(),
            soft_clip,
            monitor,
            silence: [0.0; MAX_BLOCK],
        };
        let handle = RtmpSinkHandle { shared };
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

impl Drop for RtmpSink {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl Module for RtmpSink {
    fn name(&self) -> &str {
        "RtmpSink"
    }

    fn process(&mut self, frames: usize) -> bool {
        let stopping = self.shared.is_stopping();
        for i in 0..frames {
            let (left, right) = (self.inputs.audio_left(i), self.inputs.audio_right(i));
            self.outputs.set(i, left, right);

            if !stopping {
                let (stream_left, stream_right) = if self.soft_clip {
                    (Self::soft_clip_sample(left), Self::soft_clip_sample(right))
                } else {
                    (left, right)
                };
                self.shared.push_audio(stream_left, stream_right);
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

impl SinkModule for RtmpSink {
    fn sink_block(&self) -> (&[f32], &[f32]) {
        if self.monitor {
            (self.outputs.left_block(), self.outputs.right_block())
        } else {
            (&self.silence, &self.silence)
        }
    }
}

impl RtmpSinkHandle {
    pub fn push_video_rgba(&self, frame: &[u8]) -> Result<(), String> {
        self.shared.push_video_rgba(frame)
    }

    pub fn finish(&self) -> RtmpSinkStats {
        self.shared.finish();
        self.stats()
    }

    pub fn stats(&self) -> RtmpSinkStats {
        self.shared.stats().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_exposes_ports_without_building() {
        let factory = RtmpSinkFactory;
        assert!(factory.is_sink());
        assert_eq!(factory.input_ports(), Some(&inputs::INPUTS[..]));
        assert_eq!(factory.output_ports(), Some(&outputs::OUTPUTS[..]));
    }

    #[test]
    fn default_registry_lists_rtmp_sink() {
        let registry = crate::ModuleRegistry::default();
        assert!(registry.has_type("rtmp_sink"));
        assert!(registry.is_sink("rtmp_sink"));
        assert_eq!(
            registry.factory_input_ports("rtmp_sink"),
            Some(&inputs::INPUTS[..])
        );
        assert_eq!(
            registry.factory_output_ports("rtmp_sink"),
            Some(&outputs::OUTPUTS[..])
        );
    }
}
