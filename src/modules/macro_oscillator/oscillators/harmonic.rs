//! Ported from Mutable Instruments' `plaits/dsp/oscillator/harmonic_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::{sine, BlockRamp, BlockSteps};

/// How many harmonics one oscillator renders.
pub(in crate::modules::macro_oscillator) const HARMONIC_BATCH: usize = 12;

/// A bank of [`HARMONIC_BATCH`] consecutive harmonics, starting at
/// `first`, summed with the Chebyshev recurrence: one table read per
/// sample (two above the first batch) instead of one per harmonic.
/// Harmonics fade as they near Nyquist and stop there.
pub(in crate::modules::macro_oscillator) struct HarmonicOscillator {
    first: usize,
    phase: f32,
    frequency: BlockRamp,
    amplitudes: [BlockRamp; HARMONIC_BATCH],
}

impl HarmonicOscillator {
    /// A bank whose lowest harmonic is number `first` (1 is the
    /// fundamental).
    pub fn new(first: usize) -> Self {
        Self {
            first: first.max(1),
            phase: 0.0,
            frequency: BlockRamp::new(0.0),
            amplitudes: [BlockRamp::new(0.0); HARMONIC_BATCH],
        }
    }

    /// Renders the harmonics of `frequency` (cycles per sample) at
    /// `amplitudes`, writing into `out` if this is the first batch and
    /// adding to it otherwise.
    pub fn render(&mut self, frequency: f32, amplitudes: &[f32], out: &mut [f32]) {
        let size = out.len();
        let frequency = frequency.min(0.5);
        let first = self.first as f32;
        let mut ams: [BlockSteps; HARMONIC_BATCH] = std::array::from_fn(|i| {
            let f = (frequency * (first + i as f32)).min(0.5);
            self.amplitudes[i].steps(amplitudes[i] * (1.0 - f * 2.0), size)
        });
        let frequencies = self.frequency.steps(frequency, size);

        for (o, f) in out.iter_mut().zip(frequencies) {
            self.phase += f;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let two_x = 2.0 * sine(self.phase);
            // The recurrence runs on Chebyshev polynomials of sin: term n
            // is a harmonic n at full level.
            let (mut previous, mut current) = if self.first == 1 {
                (1.0, two_x * 0.5)
            } else {
                (
                    sine(self.phase * (first - 1.0) + 0.25),
                    sine(self.phase * first),
                )
            };
            let mut sum = 0.0;
            for am in ams.iter_mut() {
                sum += am.next().unwrap_or(0.0) * current;
                let next = two_x * current - previous;
                previous = current;
                current = next;
            }
            if self.first == 1 {
                *o = sum;
            } else {
                *o += sum;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rising zero crossings of harmonic `n` alone, over 4800 samples at
    /// 0.01 cycles per sample.
    fn crossings(n: usize) -> usize {
        let first = if n > HARMONIC_BATCH {
            HARMONIC_BATCH + 1
        } else {
            1
        };
        let mut oscillator = HarmonicOscillator::new(first);
        let mut amplitudes = [0.0; HARMONIC_BATCH];
        amplitudes[n - first] = 1.0;
        let mut out = vec![0.0; 4800];
        for block in out.chunks_mut(12) {
            block.fill(0.0);
            oscillator.render(0.01, &amplitudes, block);
        }
        let peak = out[480..].iter().fold(0.0f32, |p, v| p.max(v.abs()));
        assert!(peak > 0.5 && peak < 1.01, "harmonic {n} peak {peak}");
        out[480..]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count()
    }

    #[test]
    fn each_harmonic_sounds_at_its_multiple() {
        // 4320 samples at 0.01 cycles is 43.2 cycles of the fundamental.
        for n in [1, 2, 5, 12, 13, 20] {
            let expected = 43.2 * n as f32;
            let got = crossings(n) as f32;
            assert!((got - expected).abs() <= 1.0, "harmonic {n}: {got}");
        }
    }
}
