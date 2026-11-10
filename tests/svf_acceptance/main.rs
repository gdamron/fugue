//! Acceptance tests for the block-wise parameter ramp and the state-variable
//! filter, written from the spec alone. Assumptions are marked "Assumption".

mod no_alloc;
mod ramp;
mod response;
mod stability;
mod structure;

use fugue::dsp::{Svf, SvfOutput};
use std::f64::consts::TAU;

pub const RATE: u32 = 48_000;
/// Peak amplitude of every test tone.
pub const AMP: f64 = 0.5;

/// Steady-state amplitude (RMS times sqrt 2) of each Svf output for a sine at
/// `freq_hz`, as `(lowpass, bandpass, highpass)`. Half a second is discarded
/// (many time constants even for q = 10 at 200 Hz) and the next full second is
/// measured. A sampled peak would miss at few samples per cycle; the RMS of a
/// whole number of cycles does not.
pub fn tone_amplitudes(svf: &mut Svf, freq_hz: f64) -> (f64, f64, f64) {
    let rate = RATE as f64;
    let (skip, len) = (RATE as usize / 2, RATE as usize);
    let (mut lp, mut bp, mut hp) = (0.0, 0.0, 0.0);
    for n in 0..skip + len {
        let x = (AMP * (TAU * freq_hz * n as f64 / rate).sin()) as f32;
        let SvfOutput {
            lowpass,
            bandpass,
            highpass,
        } = svf.process(x);
        if n >= skip {
            lp += (lowpass as f64).powi(2);
            bp += (bandpass as f64).powi(2);
            hp += (highpass as f64).powi(2);
        }
    }
    let amp = |sum: f64| (sum / len as f64).sqrt() * std::f64::consts::SQRT_2;
    (amp(lp), amp(bp), amp(hp))
}

/// Deterministic noise in [-0.5, 0.5).
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Self {
        Noise(seed)
    }

    pub fn sample(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1 << 24) as f32 - 0.5
    }
}
