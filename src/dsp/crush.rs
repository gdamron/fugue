//! Lo-fi reduction: fewer bits, a lower sample rate.
//!
//! These are deliberately crude. Rounding to a coarse grid adds the gritty
//! noise of an early sampler; holding each sample for several frames adds
//! the metallic, inharmonic ring of one run too slow. Both alias on
//! purpose, so neither belongs inside an oversampler.

/// Rounds a sample to a grid of `step` (mid-tread: zero stays zero, so
/// silence stays silent).
#[inline]
pub fn quantize(x: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return x;
    }
    (x / step).round() * step
}

/// The grid step for `bits` of resolution across ±1.
pub fn bit_step(bits: f32) -> f32 {
    2.0 * (-bits).exp2()
}

/// Holds each sample for a while: a sample-rate reducer.
#[derive(Clone, Copy, Debug)]
pub struct SampleHold {
    phase: f32,
    held: f32,
}

impl SampleHold {
    /// A reducer at rest at 0 that keeps its next sample.
    pub fn new() -> Self {
        Self {
            phase: 1.0,
            held: 0.0,
        }
    }

    /// Takes one sample, keeping a new one each time the phase passes a
    /// whole number. `rate` is the fraction of the full rate kept (1 keeps
    /// every sample, ¼ holds each for four frames); a fractional hold
    /// length is kept on average, so the rate sweeps smoothly.
    #[inline]
    pub fn process(&mut self, x: f32, rate: f32) -> f32 {
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held = x;
        }
        self.phase += rate.clamp(0.0, 1.0);
        self.held
    }

    /// Restarts so the next sample is kept.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for SampleHold {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_keeps_zero_and_lands_on_the_grid() {
        let step = bit_step(3.0);
        assert_eq!(step, 0.25);
        assert_eq!(quantize(0.0, step), 0.0);
        assert_eq!(quantize(0.1, step), 0.0);
        assert_eq!(quantize(0.13, step), 0.25);
        assert_eq!(quantize(-0.6, step), -0.5);
        assert_eq!(quantize(0.3, 0.0), 0.3);
    }

    #[test]
    fn full_rate_passes_every_sample_and_a_quarter_holds_four() {
        let mut hold = SampleHold::new();
        for x in [0.1, -0.2, 0.3] {
            assert_eq!(hold.process(x, 1.0), x);
        }
        let mut hold = SampleHold::new();
        hold.process(9.0, 0.5);
        hold.reset();
        let out: Vec<f32> = (0..8).map(|n| hold.process(n as f32, 0.25)).collect();
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0, 4.0, 4.0, 4.0, 4.0]);
    }
}
