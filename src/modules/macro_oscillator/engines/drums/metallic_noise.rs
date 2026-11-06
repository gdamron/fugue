//! Ported from Mutable Instruments' `plaits/dsp/drums/hi_hat.h`
//! (`SquareNoise`, `RingModNoise`) and the square and saw shapes of
//! `plaits/dsp/oscillator/oscillator.h` (MIT, Copyright 2016 Emilie
//! Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! The clangorous sources under a hi-hat: detuned square waves summed, or
//! square waves ring-modulated by saws, so no pitch stands out.

use super::filters::{BlockGlide, Rate};
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample, MAX_FREQUENCY};

/// Six square waves at inharmonic ratios of the pitch, summed: the six
/// Schmitt-trigger oscillators of a classic analog hi-hat circuit.
#[derive(Default)]
pub(super) struct SquareNoise {
    phases: [u32; 6],
}

impl SquareNoise {
    /// The oscillators' ratios to the pitch (nominally 414 Hz).
    const RATIOS: [f32; 6] = [1.0, 1.304, 1.466, 1.787, 1.932, 2.536];

    /// Renders `out` at pitch `f0`, in cycles per sample.
    pub fn render(&mut self, f0: f32, out: &mut [f32]) {
        let increments = Self::RATIOS.map(|r| ((f0 * r).min(0.499) * 4_294_967_296.0) as u32);
        for o in out.iter_mut() {
            let mut high = 0u32;
            for (phase, increment) in self.phases.iter_mut().zip(increments) {
                *phase = phase.wrapping_add(increment);
                high += *phase >> 31;
            }
            *o = 0.33 * high as f32 - 1.0;
        }
    }
}

/// Which band-limited wave a [`BlepOscillator`] draws.
#[derive(Clone, Copy)]
enum Shape {
    Square,
    Saw,
}

/// A square or saw wave at a fixed pulse width of a half, its steps
/// smoothed with a polyBLEP.
struct BlepOscillator {
    shape: Shape,
    phase: f32,
    next_sample: f32,
    high: bool,
    frequency: BlockGlide,
}

impl BlepOscillator {
    fn new(shape: Shape) -> Self {
        let mut frequency = BlockGlide::default();
        frequency.start(0.001, 1);
        frequency.next();
        Self {
            shape,
            phase: 0.5,
            next_sample: 0.0,
            high: true,
            frequency,
        }
    }

    fn render(&mut self, frequency: f32, out: &mut [f32]) {
        const PW: f32 = 0.5;
        self.frequency
            .start(frequency.clamp(0.000_001, MAX_FREQUENCY), out.len());
        let mut next_sample = self.next_sample;
        for o in out.iter_mut() {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            let frequency = self.frequency.next();
            self.phase += frequency;
            match self.shape {
                Shape::Saw => {
                    if self.phase >= 1.0 {
                        self.phase -= 1.0;
                        let t = self.phase / frequency;
                        this_sample -= this_blep_sample(t);
                        next_sample -= next_blep_sample(t);
                    }
                    next_sample += self.phase;
                }
                Shape::Square => {
                    if self.high ^ (self.phase >= PW) {
                        let t = (self.phase - PW) / frequency;
                        this_sample += this_blep_sample(t);
                        next_sample += next_blep_sample(t);
                        self.high = self.phase >= PW;
                    }
                    if self.phase >= 1.0 {
                        self.phase -= 1.0;
                        let t = self.phase / frequency;
                        this_sample -= this_blep_sample(t);
                        next_sample -= next_blep_sample(t);
                        self.high = false;
                    }
                    next_sample += if self.phase < PW { 0.0 } else { 1.0 };
                }
            }
            *o = 2.0 * this_sample - 1.0;
        }
        self.next_sample = next_sample;
    }
}

/// Three square waves, each ring-modulated by a saw far above it.
pub(super) struct RingModNoise {
    rate: Rate,
    sample_rate: f32,
    pairs: [(BlepOscillator, BlepOscillator); 3],
}

impl RingModNoise {
    /// The pairs' frequencies, in hertz, at full pitch.
    const PAIRS: [(f32, f32); 3] = [(200.0, 7_530.0), (510.0, 8_075.0), (730.0, 10_500.0)];

    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: Rate::new(sample_rate),
            sample_rate: sample_rate.max(1) as f32,
            pairs: std::array::from_fn(|_| {
                (
                    BlepOscillator::new(Shape::Square),
                    BlepOscillator::new(Shape::Saw),
                )
            }),
        }
    }

    /// Renders `out` at pitch `f0`, in cycles per sample; `square` and
    /// `saw` are scratch of the same length.
    pub fn render(&mut self, f0: f32, square: &mut [f32], saw: &mut [f32], out: &mut [f32]) {
        // Pitches well under 480 Hz pull the pairs down with them.
        let ratio = f0 / (0.01 / self.rate.at_48k(1.0) + f0);
        out.fill(0.0);
        for ((low, high), (low_hz, high_hz)) in self.pairs.iter_mut().zip(Self::PAIRS) {
            low.render(low_hz / self.sample_rate * ratio, square);
            high.render(high_hz / self.sample_rate * ratio, saw);
            for ((o, a), b) in out.iter_mut().zip(&*square).zip(&*saw) {
                *o += a * b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_squares_sum_to_seven_levels_in_range() {
        let mut noise = SquareNoise::default();
        let mut out = [0.0; 4_800];
        noise.render(414.0 / 48_000.0, &mut out);
        let mut levels: Vec<i32> = out
            .iter()
            .map(|s| ((s + 1.0) / 0.33).round() as i32)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert!(levels.len() >= 6, "{levels:?}");
        assert!(out.iter().all(|s| (-1.0..=0.99).contains(s)));
    }

    #[test]
    fn a_blep_wave_crosses_at_its_frequency_and_stays_in_range() {
        for shape in [Shape::Square, Shape::Saw] {
            let mut osc = BlepOscillator::new(shape);
            let mut out = [0.0; 48_000];
            // The first block glides up from rest.
            osc.render(100.0 / 48_000.0, &mut out[..12]);
            osc.render(100.0 / 48_000.0, &mut out);
            let rises = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            assert!((99..=101).contains(&rises), "{rises}");
            assert!(out.iter().all(|s| s.abs() <= 1.01));
        }
    }

    #[test]
    fn the_ring_modulated_noise_is_dense_and_bounded() {
        let mut noise = RingModNoise::new(48_000);
        let (mut a, mut b, mut out) = ([0.0; 480], [0.0; 480], [0.0; 480]);
        noise.render(2.0 * 414.0 / 48_000.0, &mut a, &mut b, &mut out);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / 480.0).sqrt();
        assert!(rms > 0.5, "{rms}");
        assert!(out.iter().all(|s| s.abs() <= 3.1));
    }
}
