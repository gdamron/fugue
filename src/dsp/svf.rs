//! The state-variable filter: lowpass, bandpass and highpass from one
//! resonant two-pole core.
//!
//! This is the trapezoidal-integrator ("topology-preserving") form of
//! the classic two-integrator loop, as derived by Zavalishin and written
//! up as a linear SVF by Simper. Its cutoff is exact at every frequency
//! below Nyquist, its resonance does not depend on the cutoff, and it
//! stays stable and smooth while both move at audio rate, which is what
//! a filter that is played needs.

use std::f32::consts::PI;

/// A state smaller than this is flushed to zero, so that silence does not
/// leave the filter running on slow subnormal numbers.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// The resonance range. At the low end the response is very flat; at the
/// high end the filter rings for seconds but cannot run away.
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 100.0;

/// A two-pole resonant filter with every response at once.
#[derive(Clone, Copy, Debug)]
pub struct Svf {
    sample_rate: f32,
    cutoff_hz: f32,
    q: f32,
    /// `g = tan(π·cutoff/rate)`, recomputed only when the cutoff moves.
    g: f32,
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
        let mut filter = Self {
            sample_rate: sample_rate.max(1) as f32,
            cutoff_hz: f32::NAN,
            q: f32::NAN,
            g: 0.0,
            k: 1.0,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            first: 0.0,
            second: 0.0,
        };
        filter.set(cutoff_hz, q);
        filter
    }

    /// Moves the cutoff (clamped to just below Nyquist) and the resonance
    /// (clamped to 0.1..100). Unchanged values cost nothing; values that
    /// are not numbers are ignored.
    #[inline]
    pub fn set(&mut self, cutoff_hz: f32, q: f32) {
        let cutoff = if cutoff_hz.is_nan() {
            self.cutoff_hz
        } else {
            cutoff_hz.clamp(0.0, 0.49 * self.sample_rate)
        };
        let q = if q.is_nan() {
            self.q
        } else {
            q.clamp(MIN_Q, MAX_Q)
        };
        // NaN only at construction with NaN arguments: settle somewhere.
        let cutoff = if cutoff.is_nan() { 0.0 } else { cutoff };
        let q = if q.is_nan() {
            std::f32::consts::FRAC_1_SQRT_2
        } else {
            q
        };
        let same_cutoff = cutoff.to_bits() == self.cutoff_hz.to_bits();
        if same_cutoff && q.to_bits() == self.q.to_bits() {
            return;
        }
        if !same_cutoff {
            self.cutoff_hz = cutoff;
            self.g = (PI * cutoff / self.sample_rate).tan();
        }
        self.q = q;
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    /// The cutoff in hertz.
    pub fn cutoff(&self) -> f32 {
        self.cutoff_hz
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
}

#[inline]
fn flush(state: f32) -> f32 {
    if state.abs() < SUBNORMAL_FLUSH {
        0.0
    } else {
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_1_SQRT_2, TAU};

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
    fn the_responses_add_back_to_the_input() {
        let mut filter = Svf::new(RATE, 700.0, 3.0);
        for n in 0..2_000 {
            let x = ((n * 7919) % 101) as f32 / 50.0 - 1.0;
            let out = filter.process(x);
            let sum = out.lowpass + filter.k * out.bandpass + out.highpass;
            assert!((sum - x).abs() < 1e-5);
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
}
