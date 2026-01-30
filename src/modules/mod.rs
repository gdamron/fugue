//! Modular synthesis components.
//!
//! This module contains all the building blocks for creating modular synthesis patches:
//! - [`Clock`] / [`Tempo`] - Timing and tempo control
//! - [`Oscillator`] / [`OscillatorType`] - Waveform generation
//! - [`MelodyGenerator`] / [`MelodyParams`] - Algorithmic melody generation
//! - [`Adsr`] - Envelope generator
//! - [`Vca`] - Voltage controlled amplifier
//! - [`Dac`] - Audio output
//!
//! Each module also provides a factory for self-contained construction:
//! - [`ClockFactory`], [`OscillatorFactory`], [`AdsrFactory`], [`VcaFactory`], [`MelodyFactory`]

pub mod adsr;
pub mod clock;
pub mod dac;
pub mod melody;
pub mod oscillator;
pub mod vca;

// Re-export module types
pub use adsr::Adsr;
pub use clock::{Clock, Tempo};
pub use dac::Dac;
pub use melody::{MelodyGenerator, MelodyParams};
pub use oscillator::{Oscillator, OscillatorType};
pub use vca::Vca;

// Re-export factory types
pub use adsr::AdsrFactory;
pub use clock::ClockFactory;
pub use melody::MelodyFactory;
pub use oscillator::OscillatorFactory;
pub use vca::VcaFactory;
