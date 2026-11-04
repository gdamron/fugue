//! Ported from Mutable Instruments' `plaits/dsp/oscillator/oscillator.h`
//! (the slope shape; MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's
//! block processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{
    next_integrated_blep_sample, this_integrated_blep_sample, MAX_FREQUENCY,
};

/// The lowest frequency, as a fraction of the sample rate.
const MIN_FREQUENCY: f32 = 0.000_001;

/// A band-limited slope: a ramp up for `pw` of the cycle and down for the
/// rest, so a `pw` of 0.5 is a triangle and one near 1 a saw. The corners
/// are smoothed with integrated polyBLEPs.
pub(in crate::modules::macro_oscillator) struct SlopeOscillator {
    phase: f32,
    next_sample: f32,
    /// Whether the phase is on the rising part.
    high: bool,
    frequency: BlockRamp,
    pw: BlockRamp,
}

impl SlopeOscillator {
    pub fn new() -> Self {
        Self {
            phase: 0.5,
            next_sample: 0.0,
            high: true,
            frequency: BlockRamp::new(0.001),
            pw: BlockRamp::new(0.5),
        }
    }

    /// Renders `out.len()` samples at `frequency` (cycles per sample) with
    /// the rise taking `pw` of the cycle, both glided across the block.
    pub fn render(&mut self, frequency: f32, pw: f32, out: &mut [f32]) {
        let frequency = frequency.clamp(MIN_FREQUENCY, MAX_FREQUENCY);
        let pw = pw.clamp(frequency * 2.0, 1.0 - 2.0 * frequency);
        let size = out.len();
        let frequencies = self.frequency.steps(frequency, size);
        let pws = self.pw.steps(pw, size);
        let mut next_sample = self.next_sample;

        for (o, (frequency, pw)) in out.iter_mut().zip(frequencies.zip(pws)) {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            self.phase += frequency;

            let slope_up = 1.0 / pw;
            let slope_down = 1.0 / (1.0 - pw);
            let discontinuity = (slope_up + slope_down) * frequency;
            if self.high ^ (self.phase < pw) {
                let t = (self.phase - pw) / frequency;
                this_sample -= this_integrated_blep_sample(t) * discontinuity;
                next_sample -= next_integrated_blep_sample(t) * discontinuity;
                self.high = self.phase < pw;
            }
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                let t = self.phase / frequency;
                this_sample += this_integrated_blep_sample(t) * discontinuity;
                next_sample += next_integrated_blep_sample(t) * discontinuity;
                self.high = true;
            }
            next_sample += if self.high {
                self.phase * slope_up
            } else {
                1.0 - (self.phase - pw) * slope_down
            };
            *o = 2.0 * this_sample - 1.0;
        }
        self.next_sample = next_sample;
    }
}
