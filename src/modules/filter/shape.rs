//! The filter's two-pole stage and the coefficients that shape it.
//!
//! Every response comes from one trapezoidal state-variable stage (Andrew
//! Simper / Cytomic): the lowpass, highpass and bandpass responses are its
//! taps.

use super::FilterType;

/// The stage's coefficients.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Coefficients {
    pub(super) k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coefficients {
    /// The stage for prewarped cutoff `g` and damping `k`.
    #[inline(always)]
    pub(super) fn stage(g: f32, k: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            k,
            a1,
            a2,
            a3: g * a2,
        }
    }
}

/// One stage's integrator states.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Stage {
    first: f32,
    second: f32,
}

impl Stage {
    /// Filters one sample, returning the (bandpass, lowpass) taps.
    #[inline(always)]
    pub(super) fn tick(&mut self, c: &Coefficients, input: f32) -> (f32, f32) {
        let v3 = input - self.second;
        let v1 = c.a1 * self.first + c.a2 * v3;
        let v2 = self.second + c.a2 * self.first + c.a3 * v3;
        self.first = 2.0 * v1 - self.first;
        self.second = 2.0 * v2 - self.second;
        (v1, v2)
    }

    /// The lowpass, highpass or bandpass tap for one tick.
    #[inline(always)]
    pub(super) fn classic(&mut self, c: &Coefficients, filter_type: FilterType, input: f32) -> f32 {
        let (v1, v2) = self.tick(c, input);
        match filter_type {
            FilterType::LowPass => v2,
            FilterType::HighPass => input - c.k * v1 - v2,
            _ => v1,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}
