//! Modular synthesis components.
//!
//! This module contains all the building blocks for creating modular synthesis patches:
//! - [`Clock`] / [`Tempo`] - Timing and tempo control
//! - [`Oscillator`] / [`OscillatorType`] - Waveform generation
//! - [`Lfo`] - Low frequency oscillator for modulation
//! - [`Filter`] / [`FilterType`] - Resonant filter for subtractive synthesis
//! - [`Mixer`] - Multi-channel audio mixer
//! - [`MelodyGenerator`] / [`MelodyParams`] - Algorithmic melody generation
//! - [`StepSequencer`] / [`Step`] - Deterministic step sequencer
//! - [`Adsr`] - Envelope generator
//! - [`Vca`] - Voltage controlled amplifier
//! - [`DacModule`] - Audio output sink module
//! - [`AudioDriver`] / [`AudioBackend`] - Audio output backends
//!
//! Each module also provides a factory for self-contained construction:
//! - [`ClockFactory`], [`OscillatorFactory`], [`LfoFactory`], [`FilterFactory`], [`MixerFactory`], [`AdsrFactory`], [`VcaFactory`], [`MelodyFactory`], [`StepSequencerFactory`], [`DacFactory`]

pub mod adsr;
pub mod clock;
pub mod dac;
pub mod filter;
pub mod lfo;
pub mod melody;
pub mod mixer;
pub mod oscillator;
pub mod step_sequencer;
pub mod vca;

// Re-export module types
pub use adsr::Adsr;
pub use clock::{Clock, Tempo};
pub use dac::{default_sample_rate, AudioBackend, AudioDriver, DacModule};
pub use filter::{Filter, FilterType};
pub use lfo::Lfo;
pub use melody::{MelodyGenerator, MelodyParams};
pub use mixer::Mixer;
pub use oscillator::{Oscillator, OscillatorType};
pub use step_sequencer::{Step, StepSequencer};
pub use vca::Vca;

// Re-export factory types
pub use adsr::AdsrFactory;
pub use clock::ClockFactory;
pub use dac::DacFactory;
pub use filter::FilterFactory;
pub use lfo::LfoFactory;
pub use melody::MelodyFactory;
pub use mixer::MixerFactory;
pub use oscillator::OscillatorFactory;
pub use step_sequencer::StepSequencerFactory;
pub use vca::VcaFactory;
