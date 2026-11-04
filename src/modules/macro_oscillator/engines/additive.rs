//! Ported from Mutable Instruments' `plaits/dsp/engine/additive_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.
//!
//! Additive synthesis: 24 harmonics on `audio`, and on `aux` the eight a
//! drawbar organ offers (1 to 4, 6, 8, 10, 12). `timbre` moves the peak of
//! the spectrum up through the harmonics; `morph` narrows it from a broad
//! slope to a few harmonics; `harmonics` ripples it into bumps, for
//! vocal and bell-like spectra.

use crate::dsp::sine;
use crate::modules::macro_oscillator::engine::EngineParameters;
use crate::modules::macro_oscillator::oscillators::{HarmonicOscillator, HARMONIC_BATCH};

/// Harmonics on `audio`.
const HARMONICS: usize = 2 * HARMONIC_BATCH;
/// The harmonic numbers (from 0, the fundamental) on `audio`: all of them.
const AUDIO_HARMONICS: [usize; HARMONICS] = {
    let mut harmonics = [0; HARMONICS];
    let mut i = 0;
    while i < HARMONICS {
        harmonics[i] = i;
        i += 1;
    }
    harmonics
};
/// The harmonic numbers (from 0, the fundamental) on `aux`.
const ORGAN_HARMONICS: [usize; 8] = [0, 1, 2, 3, 5, 7, 9, 11];
/// The spectrum's glide per block at 48 kHz, as a one-pole coefficient.
const SMOOTHING: f32 = 0.001;
/// A harmonic level below this (-180 dB) is silence.
const SILENT: f32 = 1e-9;
/// The rate the smoothing is tuned for.
const TUNED_RATE: f32 = 48_000.0;

pub(in crate::modules::macro_oscillator) struct AdditiveEngine {
    /// Harmonics 1–12 and 13–24 on `audio`, then 1–12 for `aux`.
    oscillators: [HarmonicOscillator; 3],
    amplitudes: [f32; HARMONICS],
    organ_amplitudes: [f32; HARMONIC_BATCH],
}

impl AdditiveEngine {
    pub fn new() -> Self {
        Self {
            oscillators: [
                HarmonicOscillator::new(1),
                HarmonicOscillator::new(HARMONIC_BATCH + 1),
                HarmonicOscillator::new(1),
            ],
            amplitudes: [0.0; HARMONICS],
            organ_amplitudes: [0.0; HARMONIC_BATCH],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let f0 = p.note_to_frequency(p.note);
        let centroid = p.timbre;
        let raw_bumps = p.harmonics;
        let raw_slope = (1.0 - 0.6 * raw_bumps) * p.morph;
        let spectrum = Spectrum {
            centroid,
            slope: 0.01 + 1.99 * raw_slope * raw_slope * raw_slope,
            bumps: 16.0 * raw_bumps * raw_bumps,
            smoothing: (SMOOTHING * TUNED_RATE * p.sample_period).min(1.0),
        };

        spectrum.update(&mut self.amplitudes, &AUDIO_HARMONICS);
        let (low, high) = self.amplitudes.split_at(HARMONIC_BATCH);
        self.oscillators[0].render(f0, low, out);
        self.oscillators[1].render(f0, high, out);

        spectrum.update(&mut self.organ_amplitudes, &ORGAN_HARMONICS);
        self.oscillators[2].render(f0, &self.organ_amplitudes, aux);
    }
}

/// A spectral envelope over a set of harmonics.
struct Spectrum {
    /// Where the peak sits across the set, 0 to 1.
    centroid: f32,
    /// How fast the levels fall away from the peak, per harmonic.
    slope: f32,
    /// How strongly the envelope ripples.
    bumps: f32,
    smoothing: f32,
}

impl Spectrum {
    /// Glides `amplitudes` at `indices` toward this envelope, then scales
    /// them to sum to about one.
    fn update(&self, amplitudes: &mut [f32], indices: &[usize]) {
        let n = indices.len() as f32 - 1.0;
        let margin = (1.0 / self.slope - 1.0) / (1.0 + self.bumps);
        let center = self.centroid * (n + margin) - 0.5 * margin;
        let mut sum = 0.001;
        for (i, &j) in indices.iter().enumerate() {
            let order = (i as f32 - center).abs() * self.slope;
            let gain = 1.0 - order;
            let gain = gain + gain.abs();
            let gain = gain * gain * (1.0 + sine(0.25 + order * self.bumps));
            let gain = gain * gain;
            let gain = gain * gain;
            // As in the original, the levels are smoothed before they are
            // normalised: not a true lowpass of the spectrum, but the
            // version that sounds right.
            amplitudes[j] += self.smoothing * (gain - amplitudes[j]);
            sum += amplitudes[j];
        }
        let scale = 1.0 / sum;
        for &j in indices {
            // A harmonic far from the peak fades toward zero forever:
            // drop it once it is inaudible, before it turns subnormal.
            let level = amplitudes[j] * scale;
            amplitudes[j] = if level < SILENT { 0.0 } else { level };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(spectrum: &Spectrum) -> [f32; HARMONICS] {
        let mut amplitudes = [0.0; HARMONICS];
        for _ in 0..20_000 {
            spectrum.update(&mut amplitudes, &AUDIO_HARMONICS);
        }
        amplitudes
    }

    fn peak(amplitudes: &[f32]) -> usize {
        (0..amplitudes.len())
            .max_by(|&a, &b| amplitudes[a].total_cmp(&amplitudes[b]))
            .unwrap()
    }

    #[test]
    fn timbre_moves_the_peak_up_the_harmonics() {
        let at = |centroid| Spectrum {
            centroid,
            slope: 0.3,
            bumps: 0.0,
            smoothing: 0.001,
        };
        let (low, high) = (settle(&at(0.1)), settle(&at(0.9)));
        assert!(peak(&low) < 5, "{}", peak(&low));
        assert!(peak(&high) > 18, "{}", peak(&high));
        let sum: f32 = high.iter().sum();
        assert!((sum - 1.0).abs() < 0.01, "{sum}");
    }

    #[test]
    fn faraway_harmonics_settle_to_exact_silence() {
        let narrow = settle(&Spectrum {
            centroid: 0.0,
            slope: 2.0,
            bumps: 0.0,
            smoothing: 0.001,
        });
        assert!(narrow.iter().all(|&a| a == 0.0 || a >= SILENT));
        assert_eq!(narrow[HARMONICS - 1], 0.0);
    }
}
