//! Ported from Mutable Instruments' `stmlib/dsp/filter.h` (`Svf`, `OnePole`
//! and their `tan` approximations; MIT, Copyright 2014 Emilie Gillet) and
//! `stmlib/dsp/dsp.h` (`SoftLimit`, `SoftClip` and the `ONE_POLE` and
//! `SLOPE` macros; MIT, Copyright 2012 Emilie Gillet). See
//! THIRD_PARTY_NOTICES.md.
//!
//! The filters the drum and noise engines are built from. A drum's modes
//! ring for seconds at Q in the thousands, past what `dsp::Svf` allows,
//! and move every sample under the drum's pitch envelope, so their cutoff
//! is a polynomial `tan` rather than a call.

use std::f32::consts::PI;

/// `x`, or 0 once it has decayed far below hearing. A decaying envelope
/// or filter state never reaches 0 in `f32`: it sticks in subnormal
/// numbers, which are slow and which a module must not output.
#[inline]
pub(super) fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-20 {
        0.0
    } else {
        x
    }
}

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

/// `tan(π·f)` to within 0.01% below 8 kHz at 48 kHz and 2.5% at 16 kHz,
/// for a cutoff that moves across the audio range.
#[inline]
pub(super) fn tan_accurate(f: f32) -> f32 {
    const P3: f32 = PI * PI * PI;
    const P5: f32 = P3 * PI * PI;
    const P7: f32 = P5 * PI * PI;
    const P9: f32 = P7 * PI * PI;
    const P11: f32 = P9 * PI * PI;
    let (a, b, c) = (3.333_314e-1 * P3, 1.333_924e-1 * P5, 5.337_406e-2 * P7);
    let (d, e) = (2.900_525e-3 * P9, 9.516_809e-3 * P11);
    let f2 = f * f;
    f * (PI + f2 * (a + f2 * (b + f2 * (c + f2 * (d + f2 * e)))))
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
    pub highpass: f32,
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
        self.first = flush(self.g * highpass + bandpass);
        let lowpass = self.g * bandpass + self.second;
        self.second = flush(self.g * bandpass + lowpass);
        Modes {
            highpass,
            lowpass,
            bandpass,
        }
    }
}

/// A one-pole filter in the same trapezoidal form: low-pass, and the
/// high-pass that is what the low-pass leaves.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct OnePoleFilter {
    g: f32,
    gi: f32,
    state: f32,
}

impl OnePoleFilter {
    /// Sets the cutoff from `g = tan(π·f)`.
    #[inline]
    pub fn set_g(&mut self, g: f32) {
        self.g = g;
        self.gi = 1.0 / (1.0 + g);
    }

    #[inline]
    pub fn lowpass(&mut self, x: f32) -> f32 {
        let lp = (self.g * x + self.state) * self.gi;
        self.state = flush(self.g * (x - lp) + lp);
        lp
    }

    #[inline]
    pub fn highpass(&mut self, x: f32) -> f32 {
        x - self.lowpass(x)
    }
}

/// Moves `state` by `coefficient` of the way toward `x`, and returns it:
/// the reference's `ONE_POLE`.
#[inline]
pub(super) fn one_pole(state: &mut f32, x: f32, coefficient: f32) -> f32 {
    *state = flush(*state + coefficient * (x - *state));
    *state
}

/// Moves `state` toward `x` by `rise` of the way when `x` is above it and
/// by `fall` when below, and returns it: the reference's `SLOPE`.
#[inline]
pub(super) fn slope(state: &mut f32, x: f32, rise: f32, fall: f32) -> f32 {
    let error = x - *state;
    *state = flush(*state + if error > 0.0 { rise } else { fall } * error);
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

    /// The per-sample step of something that takes `seconds`: the
    /// reference's `1 / (seconds · 48 kHz)`, moved to this rate so it stays
    /// between 0 and 1 however low the rate.
    pub fn per_second(self, seconds: f32) -> f32 {
        self.coefficient((1.0 / (seconds * 48_000.0)).min(1.0))
    }

    /// A one-pole coefficient given at 48 kHz, at this rate, keeping its
    /// lag (`(1 − c) / c` samples, its delay at low frequencies) the same
    /// in seconds. For a fast smoother whose difference from its input is
    /// the signal, this keeps that difference as loud;
    /// [`Rate::coefficient`] keeps its pole instead, which differs only for
    /// a coefficient far from zero.
    pub fn lag(self, at_48k: f32) -> f32 {
        let ratio = self.rate / 48_000.0;
        at_48k / (at_48k + ratio * (1.0 - at_48k))
    }

    /// The gain that keeps white noise as loud through filters fixed in
    /// hertz: its power spreads over a band that widens with the rate.
    pub fn noise_gain(self) -> f32 {
        (self.rate / 48_000.0).sqrt()
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

/// What the drum and noise engines' tests play them with.
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
            trigger_held: false,
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

    /// A strike at `note` rendered at `rate` by `engine`, as the loudness
    /// (dB RMS, `audio` and `aux`) of 25 ms windows across its first second.
    pub fn loudness_over_time<E>(
        rate: u32,
        note: f32,
        new: impl Fn(u32) -> E,
        mut render: impl FnMut(&mut E, &EngineParameters, &mut [f32], &mut [f32]),
    ) -> Vec<(f32, f32)> {
        let mut engine = new(rate);
        let p = EngineParameters {
            sample_period: 1.0 / rate as f32,
            ..params(note, false, false)
        };
        let blocks = rate as usize / ENGINE_BLOCK;
        let mut out = Vec::new();
        let mut aux = Vec::new();
        let (mut o, mut a) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
        for block in 0..blocks {
            let p = EngineParameters {
                trigger: block == 0,
                ..p
            };
            render(&mut engine, &p, &mut o, &mut a);
            out.extend_from_slice(&o);
            aux.extend_from_slice(&a);
        }
        let window = rate as usize / 40;
        let db = |x: &[f32]| 10.0 * (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).log10();
        [0.0f32, 0.05, 0.1, 0.2, 0.4]
            .iter()
            .map(|t| {
                let start = (t * rate as f32) as usize;
                let span = start..start + window;
                (db(&out[span.clone()]), db(&aux[span]))
            })
            .collect()
    }

    /// Asserts that a drum sounds alike at 44.1 and 96 kHz as at 48 kHz:
    /// within 1 dB on average across the windows, and 2 dB in each. The
    /// average is the test, since a noisy drum's single 25 ms windows move
    /// by most of a decibel with the noise alone; a gain off with the rate
    /// moves every window the same way.
    pub fn assert_rate_independent(at: impl Fn(u32) -> Vec<(f32, f32)>) {
        let reference = at(48_000);
        for rate in [44_100, 96_000] {
            let got = at(rate);
            let mut drift = (0.0f32, 0.0f32);
            for (window, (got, want)) in got.iter().zip(&reference).enumerate() {
                let delta = (got.0 - want.0, got.1 - want.1);
                assert!(
                    delta.0.abs() < 2.0 && delta.1.abs() < 2.0,
                    "at {rate} Hz, window {window}: {got:?} against {want:?} at 48 kHz"
                );
                drift = (drift.0 + delta.0, drift.1 + delta.1);
            }
            let n = reference.len() as f32;
            let drift = (drift.0 / n, drift.1 / n);
            assert!(
                drift.0.abs() < 1.0 && drift.1.abs() < 1.0,
                "at {rate} Hz the drum is {drift:?} dB off 48 kHz on average"
            );
        }
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
        for (hz, error) in [(20.0f32, 1e-4), (8_000.0, 2e-4), (16_000.0, 2.5e-2)] {
            let f = hz / 48_000.0;
            let exact = (PI * f).tan();
            assert!(
                (tan_accurate(f) / exact - 1.0).abs() < error,
                "{hz}: accurate"
            );
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

    /// A fast smoother keeps its lag in seconds at another rate.
    #[test]
    fn a_fast_smoother_keeps_its_lag_at_another_rate() {
        let lag_seconds = |c: f32, rate: f32| (1.0 - c) / c / rate;
        let fast = Rate::new(96_000).lag(0.75);
        assert!((lag_seconds(fast, 96_000.0) / lag_seconds(0.75, 48_000.0) - 1.0).abs() < 1e-5);
        assert_eq!(Rate::new(48_000).lag(0.75), 0.75);
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
