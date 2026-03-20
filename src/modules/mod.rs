//! Modular synthesis components.
//!
//! This module contains all the building blocks for creating modular synthesis setups:
//! - [`Clock`] / [`ClockControls`] - Timing and tempo control
//! - [`Oscillator`] / [`OscillatorControls`] / [`OscillatorType`] - Waveform generation
//! - [`Lfo`] / [`LfoControls`] - Low frequency oscillator for modulation
//! - [`Filter`] / [`FilterControls`] / [`FilterType`] - Resonant filter for subtractive synthesis
//! - [`Mixer`] / [`MixerControls`] - Multi-channel audio mixer
//! - [`MelodyGenerator`] / [`MelodyControls`] - Algorithmic melody generation
//! - [`StepSequencer`] / [`Step`] - Deterministic step sequencer
//! - [`Adsr`] / [`AdsrControls`] - Envelope generator
//! - [`Vca`] / [`VcaControls`] - Voltage controlled amplifier
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
pub mod sample_player;
pub mod step_sequencer;
pub mod vca;

// Re-export module types
pub use adsr::{Adsr, AdsrControls};
pub use clock::{Clock, ClockControls};

// Deprecated type alias for backward compatibility
#[allow(deprecated)]
pub use dac::{default_sample_rate, AudioBackend, AudioDriver, DacModule};
pub use filter::{Filter, FilterControls, FilterType};
pub use lfo::{Lfo, LfoControls};
pub use melody::{MelodyControls, MelodyGenerator};

pub use mixer::{Mixer, MixerControls};
pub use oscillator::{Oscillator, OscillatorControls, OscillatorType};
pub use sample_player::{SamplePlayer, SamplePlayerControls};
pub use step_sequencer::{Step, StepSequencer};
pub use vca::{Vca, VcaControls};

// Re-export factory types
pub use adsr::AdsrFactory;
pub use clock::ClockFactory;
pub use dac::DacFactory;
pub use filter::FilterFactory;
pub use lfo::LfoFactory;
pub use melody::MelodyFactory;
pub use mixer::MixerFactory;
pub use oscillator::OscillatorFactory;
pub use sample_player::SamplePlayerFactory;
pub use step_sequencer::StepSequencerFactory;
pub use vca::VcaFactory;
