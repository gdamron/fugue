//! The filter's two-pole stage and the coefficients that shape it.
//!
//! Every response comes from one trapezoidal state-variable stage (Andrew
//! Simper / Cytomic): the lowpass, highpass and bandpass responses are its
//! taps, and the EQ responses are mixes of the input with its bandpass and
//! lowpass taps, `m0·input + m1·bandpass + m2·lowpass`, with `g` and the
//! damping `k` adjusted per response. Each EQ response's magnitude is the
//! RBJ Audio EQ Cookbook filter of the same name, designed with the same
//! cutoff, gain and Q; the SVF form keeps it stable while the cutoff moves
//! at audio rate.

use std::f32::consts::{LOG2_10, PI};

use super::FilterType;
use crate::dsp::tan_quarter_turn;

/// The highest EQ cutoff, as a fraction of the sample rate.
const MAX_CUTOFF_FRACTION: f32 = 0.49;

/// The steepest a shelf gets. Past this its corner rises into a resonant
/// peak far above the plateau (+23 dB over a +12 dB shelf at q 18), which
/// is a bell's job; at 2 the corner overshoots by under 5 dB.
const MAX_SHELF_Q: f32 = 2.0;

/// The stage's coefficients and its output mix.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Coefficients {
    pub(super) k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    m0: f32,
    m1: f32,
    m2: f32,
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
            m0: 0.0,
            m1: 0.0,
            m2: 0.0,
        }
    }

    /// The EQ output for one tick of the stage.
    #[inline(always)]
    pub(super) fn mix(&self, input: f32, bandpass: f32, lowpass: f32) -> f32 {
        self.m0 * input + self.m1 * bandpass + self.m2 * lowpass
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

/// The EQ coefficients and what they were designed from, so each costly
/// part is redone only when its parameter moves.
#[derive(Clone, Copy, Debug)]
pub(super) struct EqDesign {
    /// `π / sample rate`.
    angle_per_hz: f32,
    max_cutoff: f32,
    filter_type: FilterType,
    cutoff: f32,
    boost: f32,
    q: f32,
    /// `tan(π·cutoff/rate)`.
    g: f32,
    /// `A = 10^(boost/40)` and its square root.
    a: f32,
    root_a: f32,
    pub(super) coefficients: Coefficients,
}

impl EqDesign {
    pub(super) fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            angle_per_hz: PI / rate,
            max_cutoff: MAX_CUTOFF_FRACTION * rate,
            filter_type: FilterType::LowPass,
            // NaN never compares equal, so the first design computes all.
            cutoff: f32::NAN,
            boost: f32::NAN,
            q: f32::NAN,
            g: 0.0,
            a: 1.0,
            root_a: 1.0,
            coefficients: Coefficients::default(),
        }
    }

    /// Designs `filter_type` (an EQ response) at `cutoff` Hz, `boost` dB
    /// and `q`, unless nothing moved.
    #[inline(always)]
    pub(super) fn design(&mut self, filter_type: FilterType, cutoff: f32, boost: f32, q: f32) {
        // A notch ignores boost, so a boost glide costs it nothing.
        let boost = if filter_type == FilterType::Notch && !self.boost.is_nan() {
            self.boost
        } else {
            boost
        };
        let same_shape = filter_type == self.filter_type && q == self.q;
        if same_shape && cutoff == self.cutoff && boost == self.boost {
            return;
        }
        if cutoff != self.cutoff {
            let hz = cutoff.min(self.max_cutoff);
            self.g = tan_quarter_turn(self.angle_per_hz * hz);
            self.cutoff = cutoff;
        }
        if boost != self.boost {
            self.a = (boost * (LOG2_10 / 40.0)).exp2();
            self.root_a = self.a.sqrt();
            self.boost = boost;
        }
        self.filter_type = filter_type;
        self.q = q;
        self.coefficients = self.mixed();
    }

    fn mixed(&self) -> Coefficients {
        let (a, g, q) = (self.a, self.g, self.q);
        let (g, k, m0, m1, m2) = match self.filter_type {
            FilterType::Bell => {
                let k = 1.0 / (q * a);
                (g, k, 1.0, k * (a * a - 1.0), 0.0)
            }
            FilterType::LowShelf => {
                let k = 1.0 / q.min(MAX_SHELF_Q);
                (g / self.root_a, k, 1.0, k * (a - 1.0), a * a - 1.0)
            }
            FilterType::HighShelf => {
                let k = 1.0 / q.min(MAX_SHELF_Q);
                (g * self.root_a, k, a * a, k * (1.0 - a) * a, 1.0 - a * a)
            }
            _ => {
                let k = 1.0 / q;
                (g, k, 1.0, -k, 0.0)
            }
        };
        Coefficients {
            m0,
            m1,
            m2,
            ..Coefficients::stage(g, k)
        }
    }
}
