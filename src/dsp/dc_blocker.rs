//! Removing a constant offset from a signal.
//!
//! Asymmetric distortion, rectification and some feedback paths leave a
//! signal sitting off zero. The offset is inaudible itself but eats
//! headroom, and it thumps when the sound starts or stops. A DC blocker is
//! a highpass filter set far below the lowest note: the textbook
//! first-order form, a zero at DC and a pole just inside it,
//! `y = x - x[-1] + r·y[-1]`.

use std::f32::consts::TAU;

/// The default corner, in hertz: low enough that a 20 Hz fundamental
/// loses under 2 dB, high enough that an offset settles (to 1/500 of its
/// size) within 70 ms.
pub const DC_BLOCKER_CUTOFF_HZ: f32 = 15.0;

/// A decaying state smaller than this is flushed to zero: after silence it
/// would otherwise settle into subnormal numbers, which are slow on some
/// processors.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// A highpass that removes DC and leaves the audible band alone.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    pole: f32,
    input: f32,
    output: f32,
}

impl DcBlocker {
    /// A blocker at `sample_rate` with its corner at
    /// [`DC_BLOCKER_CUTOFF_HZ`].
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cutoff(sample_rate, DC_BLOCKER_CUTOFF_HZ)
    }

    /// A blocker at `sample_rate` with its corner near `cutoff_hz` (exact
    /// for corners far below the sample rate).
    pub fn with_cutoff(sample_rate: u32, cutoff_hz: f32) -> Self {
        let pole = (-TAU * cutoff_hz.max(0.0) / sample_rate.max(1) as f32).exp();
        Self {
            pole,
            input: 0.0,
            output: 0.0,
        }
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let mut output = input - self.input + self.pole * self.output;
        if output.abs() < SUBNORMAL_FLUSH {
            output = 0.0;
        }
        self.input = input;
        self.output = output;
        output
    }

    /// Clears the filter's memory.
    pub fn reset(&mut self) {
        self.input = 0.0;
        self.output = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    #[test]
    fn a_tone_passes_unchanged() {
        let mut blocker = DcBlocker::new(RATE);
        let mut peak = 0.0f32;
        for n in 0..RATE as usize {
            let x = (std::f32::consts::TAU * 440.0 * n as f32 / RATE as f32).sin() + 0.5;
            let y = blocker.process(x);
            if n > RATE as usize / 2 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 1.0).abs() < 2e-3, "peak {peak}");
    }

    #[test]
    fn silence_settles_to_exactly_zero_and_reset_clears_the_state() {
        let mut blocker = DcBlocker::new(RATE);
        blocker.process(1.0);
        let mut last = 1.0;
        for _ in 0..RATE * 2 {
            last = blocker.process(0.0);
        }
        assert_eq!(last, 0.0);
        blocker.process(0.5);
        blocker.reset();
        assert_eq!(blocker.process(0.0), 0.0);
    }
}
