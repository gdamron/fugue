//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser
//! - `RealFft` - Radix-2 power spectrum for off-thread analysis (feature
//!   `spectrogram`)

mod allpass;
mod damper;
mod delay_line;
#[cfg(feature = "spectrogram")]
mod fft;

pub use allpass::Allpass;
pub use damper::Damper;
pub use delay_line::DelayLine;
#[cfg(feature = "spectrogram")]
pub use fft::RealFft;
