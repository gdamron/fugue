//! Ported from Mutable Instruments' `plaits/dsp/oscillator/variable_saw_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block processing.
//! See THIRD_PARTY_NOTICES.md.
//!
//! A saw with a notch cut into it at the pulse width, morphing into a
//! triangle: a thinner, more hollow saw than the plain one.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{
    next_blep_sample, next_integrated_blep_sample, this_blep_sample, this_integrated_blep_sample,
    MAX_FREQUENCY,
};

const NOTCH_DEPTH: f32 = 0.2;

pub(in crate::modules::macro_oscillator) struct VariableSawOscillator {
    phase: f32,
    next_sample: f32,
    previous_pw: f32,
    high: bool,

    frequency: BlockRamp,
    pw: BlockRamp,
    waveshape: BlockRamp,
}

impl VariableSawOscillator {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            next_sample: 0.0,
            previous_pw: 0.5,
            high: false,
            frequency: BlockRamp::new(0.01),
            pw: BlockRamp::new(0.5),
            waveshape: BlockRamp::new(0.0),
        }
    }

    /// Renders `out.len()` samples at `frequency` (cycles per sample).
    /// `waveshape` runs from the notched saw (0) to a triangle (1).
    pub fn render(&mut self, frequency: f32, pw: f32, waveshape: f32, out: &mut [f32]) {
        let size = out.len();
        let frequency = frequency.min(MAX_FREQUENCY);
        let pw = if frequency >= 0.25 {
            0.5
        } else {
            pw.clamp(frequency * 2.0, 1.0 - 2.0 * frequency)
        };

        let mut fm = self.frequency.steps(frequency, size);
        let mut pwm = self.pw.steps(pw, size);
        let mut waveshape_modulation = self.waveshape.steps(waveshape, size);

        let mut next_sample = self.next_sample;
        for sample in out.iter_mut() {
            let mut this_sample = next_sample;
            next_sample = 0.0;

            let frequency = fm.next().unwrap_or(frequency);
            let pw = pwm.next().unwrap_or(pw);
            let waveshape = waveshape_modulation.next().unwrap_or(waveshape);
            let triangle_amount = waveshape;
            let notch_amount = 1.0 - waveshape;
            let slope_up = 1.0 / pw;
            let slope_down = 1.0 / (1.0 - pw);

            self.phase += frequency;

            if !self.high && self.phase >= pw {
                let triangle_step = (slope_up + slope_down) * frequency * triangle_amount;
                let notch = (NOTCH_DEPTH + 1.0 - pw) * notch_amount;
                let t = (self.phase - pw) / (self.previous_pw - pw + frequency);
                this_sample += notch * this_blep_sample(t);
                next_sample += notch * next_blep_sample(t);
                this_sample -= triangle_step * this_integrated_blep_sample(t);
                next_sample -= triangle_step * next_integrated_blep_sample(t);
                self.high = true;
            } else if self.phase >= 1.0 {
                self.phase -= 1.0;
                let triangle_step = (slope_up + slope_down) * frequency * triangle_amount;
                let notch = (NOTCH_DEPTH + 1.0) * notch_amount;
                let t = self.phase / frequency;
                this_sample -= notch * this_blep_sample(t);
                next_sample -= notch * next_blep_sample(t);
                this_sample += triangle_step * this_integrated_blep_sample(t);
                next_sample += triangle_step * next_integrated_blep_sample(t);
                self.high = false;
            }

            let notch_saw = if self.phase < pw {
                self.phase
            } else {
                1.0 + NOTCH_DEPTH
            };
            let triangle = if self.phase < pw {
                self.phase * slope_up
            } else {
                1.0 - (self.phase - pw) * slope_down
            };
            next_sample += notch_saw * notch_amount + triangle * triangle_amount;
            self.previous_pw = pw;

            *sample = (2.0 * this_sample - 1.0) / (1.0 + NOTCH_DEPTH);
        }
        self.next_sample = next_sample;
    }
}
