//! The state-variable filter: lowpass, bandpass, highpass and notch from
//! one resonant two-pole core.
//!
//! This is the trapezoidal-integrator ("topology-preserving") form of
//! the classic two-integrator loop, as derived by Zavalishin and written
//! up as a linear SVF by Simper. Its cutoff is exact at every frequency
//! below Nyquist (the bilinear transform is prewarped there), its peak
//! height at the cutoff is `q` whatever the cutoff, and it is well
//! behaved while both move at audio rate, which is what a filter that is
//! played needs.
//!
//! Moving the cutoff costs a `tan`. [`Svf::set`] pays it only when the
//! cutoff changes; a cutoff that moves every sample should be prewarped
//! once per block with [`Svf::prewarp`], ramped, and applied with
//! [`Svf::set_prewarped`], which costs one division.

use std::f32::consts::{FRAC_1_SQRT_2, PI};

/// A state smaller than this is flushed to zero, so that silence does not
/// leave the filter running on slow subnormal numbers.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// The lowest cutoff. At zero the integrators would freeze, holding
/// whatever they last carried as a DC offset; above it they always decay.
pub const SVF_MIN_CUTOFF_HZ: f32 = 5.0;

/// The highest cutoff, as a fraction of the sample rate.
const MAX_CUTOFF_FRACTION: f32 = 0.49;

/// The resonance range. At the low end the response is very flat; at the
/// high end the filter rings for seconds but cannot run away. Every
/// output's gain at the cutoff is `q`: +40 dB at the top of the range,
/// which a module playing it will want to compensate.
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 100.0;

/// A two-pole resonant filter with every response at once.
#[derive(Clone, Copy, Debug)]
pub struct Svf {
    sample_rate: f32,
    /// The cutoff range, in hertz.
    min_cutoff_hz: f32,
    max_cutoff_hz: f32,
    /// The cutoff last given to [`Svf::set`], or NaN after
    /// [`Svf::set_prewarped`], when it is worked out from `g` on request.
    cutoff_hz: f32,
    q: f32,
    /// `g = tan(π·cutoff/rate)`.
    g: f32,
    /// The range of `g` that the cutoff range maps to.
    min_g: f32,
    max_g: f32,
    /// Damping, `1/q`.
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    /// The two integrators' states.
    first: f32,
    second: f32,
}

/// One sample's responses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfOutput {
    pub lowpass: f32,
    /// Peaks at `q` at the cutoff (not normalised to unity).
    pub bandpass: f32,
    pub highpass: f32,
}

impl SvfOutput {
    /// Everything but the cutoff.
    #[inline]
    pub fn notch(&self) -> f32 {
        self.lowpass + self.highpass
    }
}

impl Svf {
    /// A filter at `sample_rate`, cut off at `cutoff_hz` with resonance
    /// `q` (0.707 is the flattest lowpass without a peak).
    pub fn new(sample_rate: u32, cutoff_hz: f32, q: f32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        let warp = |hz: f32| (PI * hz / sample_rate).tan();
        let max_cutoff = MAX_CUTOFF_FRACTION * sample_rate;
        // Only an absurdly low rate puts the ceiling under the floor.
        let min_cutoff = SVF_MIN_CUTOFF_HZ.min(max_cutoff);
        let mut filter = Self {
            sample_rate,
            min_cutoff_hz: min_cutoff,
            max_cutoff_hz: max_cutoff,
            cutoff_hz: min_cutoff,
            q: FRAC_1_SQRT_2,
            g: warp(min_cutoff),
            min_g: warp(min_cutoff),
            max_g: warp(max_cutoff),
            k: 1.0 / FRAC_1_SQRT_2,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            first: 0.0,
            second: 0.0,
        };
        filter.recompute(filter.g, filter.q);
        filter.set(cutoff_hz, q);
        filter
    }

    /// Moves the cutoff (clamped to 5 Hz up to just below Nyquist) and
    /// the resonance (clamped to 0.1..100). Either that is not a number
    /// is ignored. A new cutoff costs a `tan`; an unchanged one nothing.
    #[inline]
    pub fn set(&mut self, cutoff_hz: f32, q: f32) {
        let mut g = self.g;
        if !cutoff_hz.is_nan() {
            let cutoff = self.clamp_cutoff(cutoff_hz);
            if cutoff.to_bits() != self.cutoff_hz.to_bits() {
                self.cutoff_hz = cutoff;
                g = self.warp(cutoff);
            }
        }
        self.update(g, q);
    }

    /// `g` for a cutoff in hertz (clamped as by [`Svf::set`]): the costly
    /// part of a cutoff change, for [`Svf::set_prewarped`].
    #[inline]
    pub fn prewarp(&self, cutoff_hz: f32) -> f32 {
        if cutoff_hz.is_nan() {
            self.g
        } else {
            self.warp(self.clamp_cutoff(cutoff_hz))
        }
    }

    /// Moves to a cutoff already prewarped by [`Svf::prewarp`] (clamped
    /// to the cutoff range), without a `tan`. `g` rises with the cutoff,
    /// so a linear ramp between two prewarped values sweeps smoothly.
    #[inline]
    pub fn set_prewarped(&mut self, g: f32, q: f32) {
        if g.is_nan() {
            self.update(self.g, q);
            return;
        }
        let g = g.clamp(self.min_g, self.max_g);
        if g.to_bits() != self.g.to_bits() {
            self.cutoff_hz = f32::NAN;
        }
        self.update(g, q);
    }

    /// The cutoff in hertz.
    pub fn cutoff(&self) -> f32 {
        if self.cutoff_hz.is_nan() {
            self.g.atan() * self.sample_rate / PI
        } else {
            self.cutoff_hz
        }
    }

    /// The resonance.
    pub fn q(&self) -> f32 {
        self.q
    }

    /// Filters one sample into every response.
    #[inline]
    pub fn process(&mut self, input: f32) -> SvfOutput {
        let v3 = input - self.second;
        let bandpass = self.a1 * self.first + self.a2 * v3;
        let lowpass = self.second + self.a2 * self.first + self.a3 * v3;
        self.first = flush(2.0 * bandpass - self.first);
        self.second = flush(2.0 * lowpass - self.second);
        SvfOutput {
            lowpass,
            bandpass,
            highpass: input - self.k * bandpass - lowpass,
        }
    }

    /// Clears the filter's memory.
    pub fn reset(&mut self) {
        self.first = 0.0;
        self.second = 0.0;
    }

    #[inline]
    fn clamp_cutoff(&self, cutoff_hz: f32) -> f32 {
        // Adding zero turns -0.0 into 0.0 before the clamp.
        (cutoff_hz + 0.0).clamp(self.min_cutoff_hz, self.max_cutoff_hz)
    }

    #[inline]
    fn warp(&self, cutoff_hz: f32) -> f32 {
        (PI * cutoff_hz / self.sample_rate).tan()
    }

    /// Recomputes the coefficients if `g` or `q` (NaN keeps it) moved.
    #[inline]
    fn update(&mut self, g: f32, q: f32) {
        let q = if q.is_nan() {
            self.q
        } else {
            q.clamp(MIN_Q, MAX_Q)
        };
        if g.to_bits() != self.g.to_bits() || q.to_bits() != self.q.to_bits() {
            self.recompute(g, q);
        }
    }

    #[inline]
    fn recompute(&mut self, g: f32, q: f32) {
        self.g = g;
        self.q = q;
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
}

/// Flushes subnormal states to zero, and recovers from an input that was
/// not a finite number instead of carrying it forever.
#[inline]
fn flush(state: f32) -> f32 {
    if state.is_finite() && state.abs() >= SUBNORMAL_FLUSH {
        state
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;

    /// The steady-state amplitude of each response to a sine at `hz`,
    /// from its RMS (a sampled peak can miss the true one by cos 30° at
    /// six samples a cycle).
    fn peaks(filter: &mut Svf, hz: f32) -> [f32; 3] {
        filter.reset();
        let mut energy = [0.0f64; 3];
        let settle = RATE as usize / 2;
        for n in 0..RATE as usize {
            let phase = (f64::from(hz) * n as f64 / f64::from(RATE)).fract();
            let out = filter.process((TAU * phase as f32).sin());
            if n >= settle {
                for (e, x) in energy
                    .iter_mut()
                    .zip([out.lowpass, out.bandpass, out.highpass])
                {
                    *e += f64::from(x) * f64::from(x);
                }
            }
        }
        energy.map(|e| (2.0 * e / (RATE as usize - settle) as f64).sqrt() as f32)
    }

    #[test]
    fn at_the_cutoff_low_and_high_pass_give_q_and_band_pass_peaks_at_q() {
        for (cutoff, q) in [(200.0, FRAC_1_SQRT_2), (1_000.0, 2.0), (8_000.0, 10.0)] {
            let mut filter = Svf::new(RATE, cutoff, q);
            let [low, band, high] = peaks(&mut filter, cutoff);
            for (name, gain) in [("low", low), ("band", band), ("high", high)] {
                assert!(
                    (gain / q - 1.0).abs() < 0.01,
                    "{cutoff} q {q}: {name} {gain}"
                );
            }
        }
    }

    #[test]
    fn low_and_high_pass_roll_off_at_12_db_per_octave() {
        let mut filter = Svf::new(RATE, 200.0, FRAC_1_SQRT_2);
        let [at_2k, ..] = peaks(&mut filter, 2_000.0);
        let [at_4k, ..] = peaks(&mut filter, 4_000.0);
        let slope = 20.0 * (at_4k / at_2k).log10();
        assert!((slope + 12.0).abs() < 0.5, "low {slope}");
        let mut filter = Svf::new(RATE, 8_000.0, FRAC_1_SQRT_2);
        let [.., at_400] = peaks(&mut filter, 400.0);
        let [.., at_800] = peaks(&mut filter, 800.0);
        let slope = 20.0 * (at_800 / at_400).log10();
        assert!((slope - 12.0).abs() < 0.5, "high {slope}");
    }

    #[test]
    fn the_notch_removes_the_cutoff_and_keeps_the_rest() {
        let mut filter = Svf::new(RATE, 1_000.0, 1.0);
        let notch = |filter: &mut Svf, hz: f32| {
            filter.reset();
            let mut energy = 0.0f64;
            for n in 0..RATE as usize {
                let phase = (f64::from(hz) * n as f64 / f64::from(RATE)).fract();
                let out = filter.process((TAU * phase as f32).sin()).notch();
                if n >= RATE as usize / 2 {
                    energy += f64::from(out) * f64::from(out);
                }
            }
            (4.0 * energy / f64::from(RATE)).sqrt() as f32
        };
        assert!(notch(&mut filter, 1_000.0) < 0.01);
        assert!((notch(&mut filter, 100.0) - 1.0).abs() < 0.02);
        assert!((notch(&mut filter, 10_000.0) - 1.0).abs() < 0.02);
    }

    #[test]
    fn away_from_the_cutoff_band_pass_follows_the_prewarped_prototype() {
        let (cutoff, q) = (1_000.0, 2.0);
        let mut filter = Svf::new(RATE, cutoff, q);
        let warp = |hz: f32| (PI * hz / RATE as f32).tan();
        for hz in [500.0, 2_000.0] {
            // |H(jw)| = w / |1 - w^2 + jw/q| with w the warped ratio (the
            // unnormalised bandpass, which peaks at q).
            let w = warp(hz) / warp(cutoff);
            let expected = w / ((1.0 - w * w).powi(2) + (w / q).powi(2)).sqrt();
            let [_, band, _] = peaks(&mut filter, hz);
            assert!(
                (band / expected - 1.0).abs() < 0.01,
                "{hz}: {band} vs {expected}"
            );
        }
    }

    #[test]
    fn sweeping_at_audio_rate_stays_stable() {
        let mut filter = Svf::new(RATE, 1_000.0, MAX_Q);
        let mut peak = 0.0f32;
        for n in 0..RATE as usize {
            let sweep = 0.5 + 0.5 * (TAU * 300.0 * n as f32 / RATE as f32).sin();
            filter.set(20.0 + sweep * 20_000.0, MAX_Q);
            let out = filter.process(if n % 500 == 0 { 1.0 } else { 0.0 });
            peak = peak.max(out.lowpass.abs()).max(out.highpass.abs());
        }
        assert!(peak.is_finite() && peak < 1_000.0, "peak {peak}");
    }

    #[test]
    fn silence_settles_to_zero_and_bad_settings_are_ignored() {
        let mut filter = Svf::new(RATE, 30.0, 0.5);
        filter.process(1.0);
        let mut out = filter.process(0.0);
        for _ in 0..RATE * 4 {
            out = filter.process(0.0);
        }
        assert_eq!((out.lowpass, out.bandpass), (0.0, 0.0));
        let g = filter.g;
        filter.set(f32::NAN, f32::NAN);
        assert_eq!((filter.g, filter.q()), (g, 0.5));
        assert!(Svf::new(RATE, f32::NAN, f32::NAN)
            .process(1.0)
            .lowpass
            .is_finite());
    }

    #[test]
    fn an_input_that_is_not_a_number_passes_and_the_filter_recovers() {
        let mut filter = Svf::new(RATE, 1_000.0, MAX_Q);
        filter.process(f32::NAN);
        filter.process(f32::INFINITY);
        for _ in 0..4 {
            filter.process(0.0);
        }
        assert!(filter.process(0.5).lowpass.is_finite());
    }

    #[test]
    fn a_closed_cutoff_still_lets_a_held_offset_decay() {
        let mut filter = Svf::new(RATE, 1_000.0, FRAC_1_SQRT_2);
        for _ in 0..1_000 {
            filter.process(1.0);
        }
        filter.set(0.0, FRAC_1_SQRT_2);
        assert_eq!(filter.cutoff(), SVF_MIN_CUTOFF_HZ);
        let mut out = filter.process(0.0);
        for _ in 0..RATE {
            out = filter.process(0.0);
        }
        assert!(out.lowpass.abs() < 1e-3, "{}", out.lowpass);
    }

    #[test]
    fn any_sample_rate_is_safe() {
        for rate in [0, 1, 10, 11] {
            let mut filter = Svf::new(rate, 1_000.0, 1.0);
            filter.set(0.0, 1.0);
            assert!(filter.process(1.0).lowpass.is_finite(), "{rate}");
        }
    }

    #[test]
    fn a_prewarped_cutoff_matches_one_set_in_hertz() {
        let mut by_hz = Svf::new(RATE, 300.0, 4.0);
        let mut by_g = by_hz;
        by_hz.set(2_500.0, 4.0);
        by_g.set_prewarped(by_g.prewarp(2_500.0), 4.0);
        assert!((by_g.cutoff() - 2_500.0).abs() < 0.5, "{}", by_g.cutoff());
        for n in 0..500 {
            let x = ((n * 7919) % 101) as f32 / 50.0 - 1.0;
            assert_eq!(by_hz.process(x), by_g.process(x));
        }
        // Out-of-range and missing values are handled as by `set`.
        by_g.set_prewarped(f32::INFINITY, f32::NAN);
        assert!(by_g.cutoff() < RATE as f32 / 2.0 && by_g.q() == 4.0);
        by_g.set_prewarped(f32::NAN, 2.0);
        assert!(by_g.cutoff() < RATE as f32 / 2.0 && by_g.q() == 2.0);
    }
}
