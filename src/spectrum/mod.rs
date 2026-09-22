//! Spectrogram analysis of running audio.
//!
//! The audio thread only copies samples into a [`SpectrumTap`]: lock-free,
//! allocation-free, and skipped entirely when nobody is watching. A
//! [`SpectrumAnalyzer`] on a control thread turns those samples into
//! spectrogram tiles for viewers, so no transform ever runs on the audio
//! callback.

mod analyzer;
mod tap;

pub use analyzer::{SpectrumAnalyzer, SpectrumConfig};
pub use tap::SpectrumTap;
