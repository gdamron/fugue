//! A first-order filter with lowpass and highpass outputs.
//!
//! The gentlest filter: 6 dB per octave, no resonance. It tames a bright
//! edge, rounds a step, thins out lows, or splits a signal into two bands
//! that sum back to the original. Written in the trapezoidal (topology-
//! preserving) form, whose cutoff is exact at every frequency below
//! Nyquist and which stays well-behaved while the cutoff moves.

use std::f32::consts::PI;

/// A state smaller than this is flushed to zero, so that silence does not
/// leave the filter running on slow subnormal numbers.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// A one-pole lowpass and highpass. The cutoff's coefficient (a `tan`) is
/// recomputed only when the cutoff changes.
#[derive(Clone, Copy, Debug)]
pub struct OnePole {
    sample_rate: f32,
    cutoff_hz: f32,
    /// `g / (1 + g)`, with `g = tan(π·cutoff/rate)`.
    coefficient: f32,
    state: f32,
}

/// One sample's two outputs. They sum to the input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnePoleOutput {
    pub lowpass: f32,
    pub highpass: f32,
}

impl OnePole {
    /// A filter at `sample_rate` with its cutoff at `cutoff_hz`.
    pub fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let mut filter = Self {
            sample_rate: sample_rate.max(1) as f32,
            cutoff_hz: f32::NAN,
            coefficient: 0.0,
            state: 0.0,
        };
        filter.set_cutoff(cutoff_hz);
        filter
    }

    /// Moves the cutoff, clamped to just below Nyquist. A cutoff equal to
    /// the current one costs nothing; one that is not a number is ignored.
    #[inline]
    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        if cutoff_hz.is_nan() {
            if self.cutoff_hz.is_nan() {
                // Construction with a NaN cutoff: start fully closed.
                self.cutoff_hz = 0.0;
                self.coefficient = 0.0;
            }
            return;
        }
        let cutoff = cutoff_hz.clamp(0.0, 0.49 * self.sample_rate);
        if cutoff.to_bits() == self.cutoff_hz.to_bits() {
            return;
        }
        self.cutoff_hz = cutoff;
        let g = (PI * cutoff / self.sample_rate).tan();
        self.coefficient = g / (1.0 + g);
    }

    /// The cutoff in hertz.
    pub fn cutoff(&self) -> f32 {
        self.cutoff_hz
    }

    /// Filters one sample into both outputs.
    #[inline]
    pub fn process(&mut self, input: f32) -> OnePoleOutput {
        let v = (input - self.state) * self.coefficient;
        let lowpass = v + self.state;
        self.state = lowpass + v;
        if self.state.abs() < SUBNORMAL_FLUSH {
            self.state = 0.0;
        }
        OnePoleOutput {
            lowpass,
            highpass: input - lowpass,
        }
    }

    /// Filters one sample, keeping the lowpass.
    #[inline]
    pub fn lowpass(&mut self, input: f32) -> f32 {
        self.process(input).lowpass
    }

    /// Filters one sample, keeping the highpass.
    #[inline]
    pub fn highpass(&mut self, input: f32) -> f32 {
        self.process(input).highpass
    }

    /// Clears the filter's memory.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    #[test]
    fn an_unchanged_cutoff_keeps_its_coefficient() {
        let mut filter = OnePole::new(RATE, 1_000.0);
        let before = filter.coefficient;
        filter.set_cutoff(1_000.0);
        assert_eq!(filter.coefficient.to_bits(), before.to_bits());
        filter.set_cutoff(1e9);
        assert!(filter.cutoff() < RATE as f32 / 2.0);
        assert!(filter.lowpass(1.0).is_finite());
        filter.set_cutoff(f32::NAN);
        assert!(filter.cutoff().is_finite());
        assert!(OnePole::new(RATE, f32::NAN).lowpass(1.0).is_finite());
    }

    #[test]
    fn silence_settles_to_exactly_zero_and_reset_clears_the_state() {
        let mut filter = OnePole::new(RATE, 20.0);
        filter.process(1.0);
        let mut last = filter.process(0.0);
        for _ in 0..RATE * 4 {
            last = filter.process(0.0);
        }
        assert_eq!(last.lowpass, 0.0);
        filter.process(0.5);
        filter.reset();
        assert_eq!(filter.lowpass(0.0), 0.0);
    }
}
