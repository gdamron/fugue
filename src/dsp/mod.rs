//! Reusable DSP primitives.
//!
//! Low-level building blocks for audio processing modules:
//! - [`DelayLine`] - Pre-allocated circular delay buffer
//! - [`Recording`] / [`GrainCloud`] / [`Stretcher`] / [`Looper`] /
//!   [`PitchShifter`] / [`Diffuser`] - A recording read as a cloud of
//!   grains, stretched in time or looped, a pitch shifter, and a smear
//! - [`ClockDivider`] / [`RisingEdge`] - Onsets read from gate inputs
//! - [`Damper`] - One-pole lowpass filter
//! - [`Allpass`] - Schroeder allpass diffuser
//! - [`LinearRamp`] - A value that glides to each new target
//! - [`BlockRamp`] - A once-per-block value spread across the block
//! - [`Svf`] - Resonant state-variable filter (lowpass, bandpass, highpass, notch)
//! - [`Rng`] - A fast seeded random number generator for the audio thread
//! - [`PinkFilter`] - Shapes white noise into pink (−3 dB/oct)
//! - [`OnePole`] - First-order lowpass and highpass
//! - [`DcBlocker`] - Removes a constant offset
//! - [`decibels_to_level`] / [`level_to_decibels`] - Fast dB conversions for per-sample use
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
mod decibels;
mod declick;
mod delay_line;
mod drive;
#[cfg(feature = "spectrogram")]
mod fft;
mod grains;
mod halfband;
mod limiter;
mod one_pole;
mod oversampler;
mod pink;
mod ramp;
mod rng;
mod sine_table;
mod svf;
mod tan;
mod tilt;
mod waveshape;

pub use allpass::Allpass;
pub use band_limited::{BandLimitedOscillator, Shape};
pub(crate) use band_limited::{corner_residual, step_residual};
pub use block_ramp::{BlockRamp, BlockSteps};
pub use crush::{bit_step, quantize, SampleHold};
pub use damper::Damper;
pub use dc_blocker::{DcBlocker, DC_BLOCKER_CUTOFF_HZ};
pub use decibels::{decibels_to_level, exp2, level_to_decibels, log2, FLOOR_DECIBELS};
#[cfg(test)]
pub(crate) use declick::DECLICK_SECONDS;
pub(crate) use declick::{declick_gain, declick_step};
pub use delay_line::DelayLine;
pub use drive::{DriveProcessor, DriveShape, REFERENCE_PEAK};
#[cfg(feature = "spectrogram")]
pub(crate) use fft::RealFft;
pub use grains::{
    share_per_frame, ClockDivider, Diffuser, GrainCloud, GrainShape, LoopShape, Looper, Onsets,
    PitchShifter, Recording, RisingEdge, StretchShape, Stretcher, MAX_GRAINS,
};
pub use limiter::LookaheadLimiter;
pub use one_pole::{OnePole, OnePoleOutput};
pub use oversampler::{Factor, Oversampler};
pub use pink::PinkFilter;
pub use ramp::LinearRamp;
pub use rng::Rng;
pub use sine_table::{cosine, sine};
pub use svf::{Svf, SvfOutput, SVF_MIN_CUTOFF_HZ};
pub use tan::tan_quarter_turn;
pub use tilt::{Tilt, TILT_PIVOT_HZ, TILT_RANGE_DB};
pub use waveshape::{Curve, Waveshaper};
