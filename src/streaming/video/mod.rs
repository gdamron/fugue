//! Background video and black fallback frames for native streaming sinks.
//!
//! Both run on their own worker threads and hand frames to a
//! [`VideoFrameTarget`]. They decode through a [`DecoderLauncher`] and pace
//! by a [`VideoClock`], so tests can replace ffmpeg and real time with fakes
//! (see `fake`).

mod clock;
mod decoder;
#[cfg(test)]
pub(crate) mod fake;
mod fallback;
mod playback;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub(crate) use clock::{SystemClock, VideoClock};
pub(crate) use decoder::{DecoderLauncher, FfmpegDecoderLauncher};
pub(crate) use fallback::{BlackVideoFallback, BlackVideoFallbackHandle};
pub(crate) use playback::{VideoPlayback, VideoPlaybackConfig, VideoPlaybackHandle};

pub(crate) type VideoFrameTarget = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync + 'static>;

/// Where video playback gets its decoder and its time.
#[derive(Clone)]
pub(crate) struct VideoRuntime {
    pub decoders: Arc<dyn DecoderLauncher>,
    pub clock: Arc<dyn VideoClock>,
}

impl VideoRuntime {
    /// ffmpeg and the real clock.
    pub(crate) fn system() -> Self {
        Self {
            decoders: Arc::new(FfmpegDecoderLauncher),
            clock: Arc::new(SystemClock),
        }
    }
}
