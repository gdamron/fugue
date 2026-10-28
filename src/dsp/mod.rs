//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser
//! - [`LinearRamp`] - A value that glides to each new target
//! - `declick_gain` - The curved fade that ends a sound without a click
//! - `RealFft` - Radix-2 power spectrum for off-thread analysis (feature
//!   `spectrogram`)

mod allpass;
mod damper;
mod declick;
mod delay_line;
#[cfg(feature = "spectrogram")]
mod fft;
mod ramp;

pub use allpass::Allpass;
pub use damper::Damper;
pub(crate) use declick::{declick_gain, declick_step, DECLICK_SECONDS};
pub use delay_line::DelayLine;
#[cfg(feature = "spectrogram")]
pub(crate) use fft::RealFft;
pub use ramp::LinearRamp;
