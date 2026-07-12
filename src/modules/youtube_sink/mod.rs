//! YouTube Live streaming preset backed by the shared RTMP/ffmpeg pipeline.
//!
//! `youtube_sink` is a composable audio tap with YouTube-compatible RTMPS,
//! codec, bitrate, and video defaults. It accepts a stream key directly or,
//! preferably, resolves one from the environment. YouTube broadcast creation
//! and scheduling remain the responsibility of YouTube Studio.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};

use super::rtmp_sink::{RtmpSink, RtmpSinkHandle, RtmpSinkStats};

mod native;

const INPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];
const OUTPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

/// Factory for the native `youtube_sink` module type.
pub struct YoutubeSinkFactory;

impl ModuleFactory for YoutubeSinkFactory {
    fn type_id(&self) -> &'static str {
        "youtube_sink"
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
        Some(&INPUTS)
    }

    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&OUTPUTS)
    }
}

/// YouTube's module implementation reuses the RTMP streaming tap.
pub type YoutubeSink = RtmpSink;
/// Runtime handle for YouTube video input, lifecycle, and diagnostics.
pub type YoutubeSinkHandle = RtmpSinkHandle;
/// Snapshot of YouTube stream delivery statistics.
pub type YoutubeSinkStats = RtmpSinkStats;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_lists_youtube_sink() {
        let registry = crate::ModuleRegistry::default();
        assert!(registry.has_type("youtube_sink"));
        assert!(registry.is_sink("youtube_sink"));
        assert_eq!(
            registry.factory_input_ports("youtube_sink"),
            Some(&INPUTS[..])
        );
        assert_eq!(
            registry.factory_output_ports("youtube_sink"),
            Some(&OUTPUTS[..])
        );
    }
}
