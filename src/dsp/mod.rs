//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser
//! - [`LinearRamp`] - A value that glides to each new target
//! - [`BlockRamp`] - A once-per-block value spread across the block
//! - [`Svf`] - Resonant state-variable filter (lowpass, bandpass, highpass, notch)
//! - [`Rng`] - A fast seeded random number generator for the audio thread
//! - [`PinkFilter`] - Shapes white noise into pink (−3 dB/oct)
//! - [`OnePole`] - First-order lowpass and highpass
//! - [`DcBlocker`] - Removes a constant offset
//! - [`LookaheadLimiter`] - A stereo-linked lookahead brickwall limiter
//! - [`Oversampler`] - Runs a process at 2× or 4× the sample rate
//! - [`BandLimitedOscillator`] - Saw, pulse, triangle and sine without aliasing, with hard sync
//! - [`sine`] / [`cosine`] - Sine and cosine from a table built at compile time
//! - [`Curve`] / [`Waveshaper`] - Distortion curves, anti-aliased where
//!   they have corners
//! - [`DriveProcessor`] - A complete drive: tone, shape, mix and level
//! - [`Tilt`] - Leans the spectrum dark or bright around a pivot
//! - [`quantize`] / [`SampleHold`] - Bit and sample-rate reduction
//! - `declick_gain` - The curved fade that ends a sound without a click
//! - `RealFft` - Radix-2 power spectrum for off-thread analysis (feature
//!   `spectrogram`)

mod allpass;
mod band_limited;
mod block_ramp;
mod crush;
mod damper;
mod dc_blocker;
mod declick;
mod delay_line;
mod drive;
#[cfg(feature = "spectrogram")]
mod fft;
mod halfband;
mod limiter;
mod one_pole;
mod oversampler;
mod pink;
mod ramp;
mod rng;
mod sine_table;
mod svf;
mod tilt;
mod waveshape;

pub use allpass::Allpass;
pub use band_limited::{BandLimitedOscillator, Shape};
pub use block_ramp::{BlockRamp, BlockSteps};
pub use crush::{bit_step, quantize, SampleHold};
pub use damper::Damper;
pub use dc_blocker::{DcBlocker, DC_BLOCKER_CUTOFF_HZ};
#[cfg(test)]
pub(crate) use declick::DECLICK_SECONDS;
pub(crate) use declick::{declick_gain, declick_step};
pub use delay_line::DelayLine;
pub use drive::{DriveProcessor, DriveShape, REFERENCE_PEAK};
#[cfg(feature = "spectrogram")]
pub(crate) use fft::RealFft;
pub use limiter::LookaheadLimiter;
pub use one_pole::{OnePole, OnePoleOutput};
pub use oversampler::{Factor, Oversampler};
pub use pink::PinkFilter;
pub use ramp::LinearRamp;
pub use rng::Rng;
pub use sine_table::{cosine, sine};
pub use svf::{Svf, SvfOutput, SVF_MIN_CUTOFF_HZ};
pub use tilt::{Tilt, TILT_PIVOT_HZ, TILT_RANGE_DB};
pub use waveshape::{Curve, Waveshaper};
