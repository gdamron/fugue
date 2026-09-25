//! Spectrogram analysis of running audio.
//!
//! The audio thread only copies samples into the master tap: lock-free,
//! allocation-free, and skipped entirely when nobody is reading. A
//! [`SpectrumAnalyzer`] on a control thread turns a [`SpectrumReader`]'s
//! samples into spectrogram tiles for viewers, so no transform ever runs on
//! the audio callback.

mod analyzer;
mod config;
mod tap;

pub use analyzer::SpectrumAnalyzer;
pub use config::{SpectrumConfig, MAX_FFT_SIZE, MAX_FRAMES_PER_TILE, MAX_HISTORY_FRAMES};
pub(crate) use tap::SpectrumTap;
pub use tap::{SpectrumReader, TapRead};
