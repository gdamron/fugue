//! Ported from Mutable Instruments' `stmlib/dsp/filter.h` (`Svf` and its
//! `tan` approximations) and `stmlib/dsp/dsp.h` (`SoftLimit`,
//! `SoftClip` and the `ONE_POLE` macro) (MIT, Copyright 2014 Emilie Gillet). See
//! THIRD_PARTY_NOTICES.md.
//!
//! The filters the drums are built from. A drum's modes ring for seconds at
//! Q in the thousands, past what `dsp::Svf` allows, and move every sample
//! under the drum's pitch envelope, so their cutoff is a polynomial `tan`
//! rather than a call.

use std::f32::consts::PI;

/// `tan(π·f)` for `f` a fraction of the sample rate, accurate below about
/// 8 kHz at 48 kHz: one multiply-add, for a cutoff that moves every sample.
#[inline]
pub(super) fn tan_dirty(f: f32) -> f32 {
    const A: f32 = 3.736e-1 * PI * PI * PI;
    f * (PI + A * f * f)
}

/// `tan(π·f)`, accurate across 16 Hz to 16 kHz at 48 kHz.
#[inline]
pub(super) fn tan_fast(f: f32) -> f32 {
    const A: f32 = 3.260e-1 * PI * PI * PI;
    const B: f32 = 1.823e-1 * PI * PI * PI * PI * PI;
    let f2 = f * f;
    f * (PI + f2 * (A + B * f2))
}

/// A gentle saturation, exact near zero and reaching ±1 at ±3.
#[inline]
pub(super) fn soft_limit(x: f32) -> f32 {
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// [`soft_limit`], held at ±1 beyond ±3.
#[inline]
pub(super) fn soft_clip(x: f32) -> f32 {
    if x < -3.0 {
        -1.0
    } else if x > 3.0 {
        1.0
    } else {
        soft_limit(x)
    }
}

/// One sample of a [`ModeFilter`]: every response at once.
#[derive(Clone, Copy, Debug)]
pub(super) struct Modes {
    pub lowpass: f32,
    /// Peaks at the filter's Q at its cutoff.
    pub bandpass: f32,
}

/// A two-pole state-variable filter with no limit on its Q.
#[derive(Clone, Copy, Debug)]
pub(super) struct ModeFilter {
    g: f32,
    r: f32,
    h: f32,
    first: f32,
    second: f32,
}

impl Default for ModeFilter {
    fn default() -> Self {
        let mut filter = Self {
            g: 0.0,
            r: 0.0,
            h: 0.0,
            first: 0.0,
            second: 0.0,
        };
        filter.set_g_q(tan_dirty(0.01), 100.0);
        filter
    }
}

impl ModeFilter {
    /// Sets the cutoff from `g = tan(π·f)` and the resonance `q`.
    #[inline]
    pub fn set_g_q(&mut self, g: f32, q: f32) {
        self.g = g;
        self.r = 1.0 / q;
        self.h = 1.0 / (1.0 + self.r * g + g * g);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> Modes {
        let highpass = (x - self.r * self.first - self.g * self.first - self.second) * self.h;
        let bandpass = self.g * highpass + self.first;
        self.first = self.g * highpass + bandpass;
        let lowpass = self.g * bandpass + self.second;
        self.second = self.g * bandpass + lowpass;
        Modes { lowpass, bandpass }
    }
}

/// Moves `state` by `coefficient` of the way toward `x`, and returns it:
/// the reference's `ONE_POLE`.
#[inline]
pub(super) fn one_pole(state: &mut f32, x: f32, coefficient: f32) -> f32 {
    *state += coefficient * (x - *state);
    *state
}

/// A per-sample one-pole coefficient given at 48 kHz, moved to `rate`, so
/// a smoothing or decay takes the same time at any sample rate.
pub(super) fn per_sample(coefficient: f32, rate: f32) -> f32 {
    1.0 - (1.0 - coefficient).powf(48_000.0 / rate)
}

/// The reference's constants are for 48 kHz; this turns them into the
/// same times at the running rate.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rate {
    rate: f32,
}

impl Rate {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: sample_rate.max(1) as f32,
        }
    }

    /// Whole samples in `seconds`.
    pub fn samples(self, seconds: f32) -> usize {
        (seconds * self.rate) as usize
    }

    /// The per-sample step of something that takes `seconds`.
    pub fn per_second(self, seconds: f32) -> f32 {
        1.0 / (seconds * self.rate)
    }

    /// A one-pole coefficient given at 48 kHz, at this rate.
    pub fn coefficient(self, at_48k: f32) -> f32 {
        per_sample(at_48k, self.rate)
    }

    /// A frequency in cycles per sample, as a fraction of 48 kHz, for the
    /// reference's pitch-dependent amounts and resonances.
    pub fn at_48k(self, frequency: f32) -> f32 {
        frequency * self.rate / 48_000.0
    }
}

/// A value that moves linearly from where it was to a new target across a
/// block, as the reference's parameter interpolator does.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BlockGlide {
    value: f32,
    step: f32,
}

impl BlockGlide {
    #[inline]
    pub fn start(&mut self, target: f32, frames: usize) {
        self.step = (target - self.value) / frames.max(1) as f32;
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        self.value += self.step;
        self.value
    }
}

/// What the drums' tests play them with.
#[cfg(test)]
pub(super) mod testing {
    use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

    pub const RATE: u32 = 48_000;

    /// Defaults at `note`, struck (`trigger`) or left un-triggered.
    pub fn params(note: f32, trigger: bool, untriggered: bool) -> EngineParameters {
        EngineParameters {
            note,
            sample_period: 1.0 / RATE as f32,
            harmonics: 0.5,
            harmonics_target: 0.5,
            timbre: 0.5,
            morph: 0.5,
            accent: 0.8,
            trigger,
            untriggered,
            key: 0,
            scale: 0,
        }
    }

    /// `blocks` blocks of `render`'s `audio` and `aux`.
    pub fn play(
        mut render: impl FnMut(&EngineParameters, &mut [f32], &mut [f32]),
        p: &EngineParameters,
        blocks: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
        let (mut outs, mut auxes) = (Vec::new(), Vec::new());
        for _ in 0..blocks {
            render(p, &mut out, &mut aux);
            outs.extend_from_slice(&out);
            auxes.extend_from_slice(&aux);
        }
        (outs, auxes)
    }

    /// One strike at `note`, then `seconds` left to ring.
    pub fn strike(
        mut render: impl FnMut(&EngineParameters, &mut [f32], &mut [f32]),
        p: EngineParameters,
        seconds: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut out, mut aux) = play(&mut render, &EngineParameters { trigger: true, ..p }, 1);
        let blocks = (seconds * RATE as f32) as usize / ENGINE_BLOCK;
        let (o, a) = play(
            &mut render,
            &EngineParameters {
                trigger: false,
                ..p
            },
            blocks,
        );
        out.extend(o);
        aux.extend(a);
        (out, aux)
    }

    pub fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tan_approximations_follow_tan_in_the_audio_range() {
        for hz in [20.0f32, 200.0, 2_000.0, 8_000.0] {
            let f = hz / 48_000.0;
            let exact = (PI * f).tan();
            assert!((tan_fast(f) / exact - 1.0).abs() < 2e-3, "{hz}: fast");
            assert!((tan_dirty(f) / exact - 1.0).abs() < 2e-2, "{hz}: dirty");
        }
    }

    #[test]
    fn soft_clip_is_continuous_and_bounded() {
        assert_eq!(soft_clip(3.0), 1.0);
        assert!((soft_limit(3.0) - 1.0).abs() < 1e-6);
        assert_eq!(soft_clip(-10.0), -1.0);
        assert!((soft_clip(0.01) - 0.01).abs() < 1e-4);
    }

    #[test]
    fn a_mode_rings_at_its_cutoff_with_a_high_q() {
        let mut mode = ModeFilter::default();
        let f = 100.0 / 48_000.0;
        mode.set_g_q(tan_fast(f), 2_000.0);
        let mut x = Vec::new();
        for i in 0..48_000 {
            x.push(mode.process(if i == 0 { 1.0 } else { 0.0 }).bandpass);
        }
        // After a second it still rings, at about 100 crossings per second.
        let late = x[24_000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(late > 1e-3 * x[..480].iter().fold(0.0f32, |m, s| m.max(s.abs())));
        let crossings = x[24_000..]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((49..=51).contains(&crossings), "{crossings}");
    }

    #[test]
    fn a_coefficient_moved_to_another_rate_takes_the_same_time() {
        assert!((per_sample(0.01, 48_000.0) - 0.01).abs() < 1e-7);
        // Half the samples per second: twice the step, near enough.
        let c = per_sample(0.01, 24_000.0);
        assert!(((1.0 - c) - 0.99f32 * 0.99).abs() < 1e-6);
    }

    #[test]
    fn a_block_glide_lands_on_its_target() {
        let mut glide = BlockGlide::default();
        glide.start(1.0, 12);
        let values: Vec<f32> = (0..12).map(|_| glide.next()).collect();
        assert!((values[11] - 1.0).abs() < 1e-6);
        assert!(values.windows(2).all(|w| w[1] > w[0]));
    }
}
