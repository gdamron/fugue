//! Pink noise: white noise shaped to fall 3 dB per octave.
//!
//! Pink noise has equal power in every octave, which the ear hears as even
//! across the range where white noise sounds bright and hissy. A 1/f power
//! slope has no finite-order filter, but a cascade of first-order sections
//! with poles and zeros alternating at equal ratios comes close: each pole
//! starts a −6 dB/oct stretch that the next zero, an octave up, ends, so
//! the response averages −3 dB/oct with a small ripple.
//!
//! This is the textbook interleaved pole/zero approximation of a 1/f
//! filter. Pole and zero frequencies are placed in hertz and mapped to the
//! sample rate once, at construction, so the slope is the same at every
//! rate, up to about a third of it: closer to Nyquist the mapping bends
//! the response.

use std::f64::consts::TAU;

/// Pole/zero pairs in the cascade.
const SECTIONS: usize = 6;
/// The first pole, in hertz. Below it the response levels off, so the
/// noise carries no runaway rumble.
const LOWEST_POLE_HZ: f64 = 10.0;
/// Each pole sits two octaves above the last, with a zero an octave above
/// each pole.
const POLE_RATIO: f64 = 4.0;
/// The RMS of the output when fed full-scale uniform white noise (RMS
/// `1/√3`). Pink noise is close to Gaussian, with peaks about five times
/// its RMS over a minute, so this keeps them inside ±1.
const OUTPUT_RMS: f64 = 0.18;
/// The RMS of uniform white noise in `[-1, 1)`.
const WHITE_RMS: f64 = 0.577_350_269_189_625_8;

/// One first-order section: `y = x - zero·x[-1] + pole·y[-1]`.
#[derive(Clone, Copy, Debug, Default)]
struct Section {
    zero: f32,
    pole: f32,
    input: f32,
    output: f32,
}

/// A filter that turns white noise into pink. The audio thread owns it: no
/// allocation, no locks.
#[derive(Clone, Debug)]
pub struct PinkFilter {
    sections: [Section; SECTIONS],
    gain: f32,
}

impl PinkFilter {
    /// A pinking filter for `sample_rate`, scaled so full-scale uniform
    /// white noise in gives pink noise of RMS 0.18 out.
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate.max(1));
        let mut sections = [Section::default(); SECTIONS];
        for (k, section) in sections.iter_mut().enumerate() {
            let pole_hz = LOWEST_POLE_HZ * POLE_RATIO.powi(k as i32);
            let zero_hz = pole_hz * POLE_RATIO.sqrt();
            section.pole = (-TAU * pole_hz / rate).exp() as f32;
            section.zero = (-TAU * zero_hz / rate).exp() as f32;
        }
        let mut filter = Self {
            sections,
            gain: 1.0,
        };
        filter.gain = (OUTPUT_RMS / (WHITE_RMS * filter.impulse_energy(rate).sqrt())) as f32;
        filter.reset();
        filter
    }

    /// Shapes one sample of white noise.
    #[inline]
    pub fn process(&mut self, white: f32) -> f32 {
        let mut x = white;
        for section in &mut self.sections {
            let y = x - section.zero * section.input + section.pole * section.output;
            section.input = x;
            section.output = y;
            x = y;
        }
        x * self.gain
    }

    /// Clears the filter's memory.
    pub fn reset(&mut self) {
        for section in &mut self.sections {
            section.input = 0.0;
            section.output = 0.0;
        }
    }

    /// The sum of the squared impulse response (unscaled): the filter's
    /// power gain on white noise. Run once at construction, long enough for
    /// the slowest pole to die away.
    fn impulse_energy(&mut self, rate: f64) -> f64 {
        self.reset();
        let frames = (rate * 2.0) as usize;
        let mut energy = 0.0;
        for frame in 0..frames {
            let impulse = if frame == 0 { 1.0 } else { 0.0 };
            let value = f64::from(self.process(impulse));
            energy += value * value;
        }
        energy
    }

    /// The filter's gain at `hz` (scaled), from its pole/zero positions.
    #[cfg(test)]
    fn magnitude(&self, hz: f64, rate: f64) -> f64 {
        let w = TAU * hz / rate;
        let (cos, sin) = (w.cos(), w.sin());
        let mut gain = f64::from(self.gain);
        for section in &self.sections {
            let factor = |c: f32| {
                let re = 1.0 - f64::from(c) * cos;
                let im = f64::from(c) * sin;
                (re * re + im * im).sqrt()
            };
            gain *= factor(section.zero) / factor(section.pole);
        }
        gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Rng;

    /// The response stays within ±1 dB of a −3 dB/oct line from 50 Hz to
    /// 10 kHz, or to a third of the sample rate where that is lower.
    #[test]
    fn the_response_falls_three_db_per_octave() {
        for rate in [22_050u32, 32_000, 44_100, 48_000, 96_000, 192_000] {
            let filter = PinkFilter::new(rate);
            let rate = f64::from(rate);
            let top = (rate / 3.0).min(10_000.0);
            let reference = 20.0 * filter.magnitude(1_000.0, rate).log10();
            let mut hz = 50.0;
            while hz <= top {
                let level = 20.0 * filter.magnitude(hz, rate).log10();
                let line = reference - 3.0103 * (hz / 1_000.0).log2();
                assert!(
                    (level - line).abs() < 1.0,
                    "{rate} Hz: {hz:.0} Hz is {:.2} dB off",
                    level - line
                );
                hz *= 2f64.powf(1.0 / 12.0);
            }
        }
    }

    #[test]
    fn full_scale_white_noise_gives_pink_of_the_set_level() {
        let mut filter = PinkFilter::new(48_000);
        let mut rng = Rng::new(3);
        let n = 60 * 48_000;
        let mut squares = 0.0f64;
        let mut peak = 0.0f32;
        for _ in 0..n {
            let value = filter.process(rng.bipolar());
            squares += f64::from(value) * f64::from(value);
            peak = peak.max(value.abs());
        }
        let rms = (squares / n as f64).sqrt();
        assert!((rms - OUTPUT_RMS).abs() < 0.02, "rms {rms}");
        assert!(peak < 1.0, "peak {peak}");
    }
}
