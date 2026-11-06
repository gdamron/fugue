//! One side's allpass chain, with its feedback loop solved in the sample.
//!
//! Every stage is linear for a given state (`a·x + b`, see
//! [`FirstOrderAllpass::response`]), so the chain's output tapped after
//! stage n is `Aₙ·x + Bₙ`. The wet signal is a blend of taps (one tap, or two
//! while `stage_count` crossfades), `A·x + B`, and with feedback `k` around
//! the chain the input is `x = s·dry + k·wet`. Solving the two together
//! gives `wet = (A·s·dry + B) / (1 − k·A)` with no sample of delay in the
//! loop: the notches stay where the stages put them. `|A| ≤ 1` and
//! `|k| ≤ 0.95`, so the division never nears zero
//! (`|A| ≤ 1` holds while the tap weights are each at least 0 and sum to at
//! most 1, as a crossfade keeps them).

use crate::dsp::FirstOrderAllpass;

use super::controls::MOST_STAGES;

/// One side's stages and their coefficient, gliding between updates.
pub(super) struct Chain {
    stages: [FirstOrderAllpass; MOST_STAGES],
    coefficient: f32,
    step: f32,
}

impl Chain {
    pub(super) fn new() -> Self {
        Self {
            stages: [FirstOrderAllpass::new(); MOST_STAGES],
            coefficient: 0.5,
            step: 0.0,
        }
    }

    /// Glides the coefficient to `target` over the next `frames` samples
    /// (or jumps there, for `frames` 0).
    pub(super) fn retarget(&mut self, target: f32, frames: usize) {
        if frames == 0 {
            self.coefficient = target;
            self.step = 0.0;
        } else {
            self.step = (target - self.coefficient) / frames as f32;
        }
    }

    /// One sample: `dry` scaled by `scale` into the stages, `feedback` of
    /// the wet signal back in, and the wet signal (the taps weighted by
    /// `weights`, up to stage `reach`) out.
    #[inline]
    pub(super) fn tick(
        &mut self,
        dry: f32,
        scale: f32,
        feedback: f32,
        weights: &[f32; MOST_STAGES],
        reach: usize,
    ) -> f32 {
        self.coefficient += self.step;
        let coefficient = self.coefficient;
        let (mut tap_a, mut tap_b) = (1.0f32, 0.0f32);
        let (mut wet_a, mut wet_b) = (0.0f32, 0.0f32);
        for (stage, &weight) in self.stages[..reach].iter().zip(weights) {
            let (a, b) = stage.response(coefficient);
            tap_a *= a;
            tap_b = a * tap_b + b;
            wet_a += weight * tap_a;
            wet_b += weight * tap_b;
        }
        let input = scale * dry;
        let wet = (wet_a * input + wet_b) / (1.0 - feedback * wet_a);
        let mut signal = input + feedback * wet;
        // Every stage runs, so one a crossfade brings in is already warm.
        for stage in &mut self.stages {
            signal = stage.process(signal, coefficient);
        }
        wet
    }
}
