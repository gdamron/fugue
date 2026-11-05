//! Ported from Mutable Instruments' `plaits/dsp/oscillator/oscillator.h`
//! (the impulse train shape; MIT, Copyright 2016 Emilie Gillet), adapted
//! to Fugue's block processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample, MAX_FREQUENCY};

/// The lowest frequency, in cycles per sample.
const MIN_FREQUENCY: f32 = 0.000_001;

/// A band-limited impulse train: a band-limited saw, differentiated and
/// gently low-passed. The glottal pulse a voice starts from.
pub(in crate::modules::macro_oscillator) struct ImpulseTrainOscillator {
    phase: f32,
    next_sample: f32,
    lowpass: f32,
    previous: f32,
    frequency: BlockRamp,
}

impl ImpulseTrainOscillator {
    pub fn new() -> Self {
        Self {
            phase: 0.5,
            next_sample: 0.0,
            lowpass: 1.0,
            previous: 0.0,
            frequency: BlockRamp::new(0.001),
        }
    }

    /// Writes a block at `frequency` (cycles per sample) into `out`.
    pub fn render(&mut self, frequency: f32, out: &mut [f32]) {
        let frequency = frequency.clamp(MIN_FREQUENCY, MAX_FREQUENCY);
        let steps = self.frequency.steps(frequency, out.len());
        let mut next_sample = self.next_sample;
        for (sample, frequency) in out.iter_mut().zip(steps) {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            self.phase += frequency;
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
                let t = self.phase / frequency;
                this_sample -= this_blep_sample(t);
                next_sample -= next_blep_sample(t);
            }
            next_sample += self.phase;
            self.lowpass += 0.25 * ((self.previous - this_sample) - self.lowpass);
            *sample = 4.0 * self.lowpass;
            self.previous = this_sample;
        }
        self.next_sample = next_sample;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pulse_per_cycle_and_no_offset() {
        let mut osc = ImpulseTrainOscillator::new();
        let mut x = vec![0.0; 48_000];
        for block in x.chunks_mut(12) {
            osc.render(100.0 / 48_000.0, block);
        }
        let x = &x[4_800..];
        let peak = x.iter().fold(0.0f32, |m, v| m.max(*v));
        let pulses = x
            .windows(2)
            .filter(|w| w[0] < 0.5 * peak && w[1] >= 0.5 * peak)
            .count();
        assert!((pulses as i32 - 90).abs() <= 1, "{pulses}");
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 1e-3, "{mean}");
    }
}
