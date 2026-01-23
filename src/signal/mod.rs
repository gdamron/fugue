//! Signal types that flow between modules in the synthesis graph.
//!
//! This module provides the core data types for audio and control signals:
//! - [`Audio`] - Real-time audio-rate signals (waveforms, CV, gates)
//! - [`Control`] - Thread-safe user parameters (knobs, buttons)
//! - [`ClockSignal`] - Timing information (beats, measures)
//! - [`FrequencySignal`] - Pitch information in Hz

mod audio;
mod clock_signal;
mod control;
mod frequency_signal;

pub use audio::Audio;
pub use clock_signal::ClockSignal;
pub use control::Control;
pub use frequency_signal::FrequencySignal;

/// Alias for [`Audio`] (deprecated, use `Audio` instead).
pub type AudioSignal = Audio;
/// Alias for [`Audio`] when used as control voltage (deprecated, use `Audio` instead).
pub type ControlSignal = Audio;
/// Alias for [`Audio`] when used as a gate signal (deprecated, use `Audio` instead).
pub type GateSignal = Audio;
/// Alias for [`Audio`] when used as a trigger signal (deprecated, use `Audio` instead).
pub type TriggerSignal = Audio;
