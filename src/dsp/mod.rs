//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser
//! - [`LinearRamp`] - A value that glides to each new target
//! - [`Rng`] - A fast seeded random number generator for the audio thread
//! - [`PinkFilter`] - Shapes white noise into pink (−3 dB/oct)
//! - [`OnePole`] - First-order lowpass and highpass
//! - [`DcBlocker`] - Removes a constant offset
//! - [`Oversampler`] - Runs a process at 2× or 4× the sample rate
//! - `declick_gain` - The curved fade that ends a sound without a click
//! - `RealFft` - Radix-2 power spectrum for off-thread analysis (feature
//!   `spectrogram`)

mod allpass;
mod damper;
mod dc_blocker;
mod declick;
mod delay_line;
#[cfg(feature = "spectrogram")]
mod fft;
mod halfband;
mod one_pole;
mod oversampler;
mod pink;
mod ramp;
mod rng;

pub use allpass::Allpass;
pub use damper::Damper;
pub use dc_blocker::{DcBlocker, DC_BLOCKER_CUTOFF_HZ};
#[cfg(test)]
pub(crate) use declick::DECLICK_SECONDS;
pub(crate) use declick::{declick_gain, declick_step};
pub use delay_line::DelayLine;
#[cfg(feature = "spectrogram")]
pub(crate) use fft::RealFft;
pub use one_pole::{OnePole, OnePoleOutput};
pub use oversampler::{Factor, Oversampler};
pub use pink::PinkFilter;
pub use ramp::LinearRamp;
pub use rng::Rng;
