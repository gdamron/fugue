//! Signal types that flow between modules in the synthesis graph.
//!
//! This module provides the core data types for audio and control signals:
//! - [`Audio`] - Real-time audio-rate signals (waveforms, CV, gates)
//! - [`ClockSignal`] - Timing information (beats, measures)
//! - [`FrequencySignal`] - Pitch information in Hz

mod audio;
mod clock_signal;
mod frequency_signal;

pub use audio::Audio;
pub use clock_signal::ClockSignal;
pub use frequency_signal::FrequencySignal;

/// Alias for [`Audio`] (deprecated, use `Audio` instead).
pub type AudioSignal = Audio;
