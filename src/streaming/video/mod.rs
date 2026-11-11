//! Background video and black fallback frames for native streaming sinks.

mod decoder;
mod fallback;
mod playback;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub(crate) use fallback::{BlackVideoFallback, BlackVideoFallbackHandle};
pub(crate) use playback::{VideoPlayback, VideoPlaybackConfig, VideoPlaybackHandle};

pub(crate) type VideoFrameTarget = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync + 'static>;
