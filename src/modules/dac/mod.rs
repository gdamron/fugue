//! DAC (Digital-to-Analog Converter) and audio output infrastructure.
//!
//! This module provides:
//! - [`DacModule`] - A sink module that collects audio for output
//! - [`DacFactory`] - Factory for creating DacModule instances
//! - [`AudioDriver`] - cpal-based audio output backend
//! - [`AudioBackend`] - Trait for custom audio backends
//! - [`default_sample_rate`] - Get the system's audio sample rate

#[cfg(not(target_arch = "wasm32"))]
mod driver;
#[cfg(target_arch = "wasm32")]
mod driver_wasm;
mod inputs;
mod module;
mod outputs;

#[cfg(not(target_arch = "wasm32"))]
pub use driver::{default_sample_rate, AudioBackend, AudioDriver};
#[cfg(target_arch = "wasm32")]
pub use driver_wasm::{default_sample_rate, AudioBackend, AudioDriver};
pub use module::{DacFactory, DacModule};
