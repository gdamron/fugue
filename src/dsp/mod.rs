//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser

mod allpass;
mod damper;
mod delay_line;

pub use allpass::Allpass;
pub use damper::Damper;
pub use delay_line::DelayLine;
