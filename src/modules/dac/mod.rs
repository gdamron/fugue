//! DAC (Digital-to-Analog Converter) and audio output infrastructure.
//!
//! This module provides:
//! - [`DacModule`] - A sink module that collects audio for output
//! - [`DacFactory`] - Factory for creating DacModule instances
//! - [`AudioDriver`] - cpal-based audio output backend
//! - [`AudioBackend`] - Trait for custom audio backends
//! - [`default_sample_rate`] - Get the system's audio sample rate

mod driver;
mod inputs;
mod module;

pub use driver::{default_sample_rate, AudioBackend, AudioDriver};
pub use module::{DacFactory, DacModule};
