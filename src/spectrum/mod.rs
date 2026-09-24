//! Spectrogram analysis of running audio.
//!
//! The audio thread only copies samples into a [`SpectrumTap`]: lock-free,
//! allocation-free, and skipped entirely when nobody is watching. A
//! [`SpectrumAnalyzer`] on a control thread turns those samples into
//! spectrogram tiles for viewers, so no transform ever runs on the audio
//! callback.

mod analyzer;
mod tap;

pub use analyzer::{
    SpectrumAnalyzer, SpectrumConfig, MAX_FFT_SIZE, MAX_FRAMES_PER_TILE, MAX_HISTORY_FRAMES,
};
pub use tap::{SpectrumReader, SpectrumTap};
