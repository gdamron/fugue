//! Ported from Mutable Instruments' `plaits/dsp/engine/fm_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.
//!
//! Classic two-operator FM, run at four times the sample rate. `harmonics`
//! steps the modulator's frequency ratio through a set of plateaus (simple
//! ratios, detuned unisons and octaves, a few inharmonic bell ratios);
//! `timbre` sets the modulation index; `morph` adds feedback, to the
//! modulator's own phase above the middle (toward noise) and to its
//! frequency below it (rougher, growling). `aux` is a sine an octave down,
//! phase-modulated by the carrier.

use super::tables::{tables, Tables, RATIO_STEPS};
use crate::dsp::{sine, BlockRamp, Factor, Oversampler};
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

/// The oversampling factor.
const OVERSAMPLING: usize = 4;
/// `OVERSAMPLING` as a pitch offset, in semitones.
const OVERSAMPLING_SEMITONES: f32 = 24.0;

pub(in crate::modules::macro_oscillator) struct FmEngine {
    /// Phases in cycles, at the oversampled rate.
    carrier_phase: f32,
    modulator_phase: f32,
    sub_phase: f32,
    carrier_frequency: BlockRamp,
    modulator_frequency: BlockRamp,
    amount: BlockRamp,
    feedback: BlockRamp,
    /// The carrier, smoothed, fed back into the modulator.
    previous_sample: f32,
    carrier_high: [f32; OVERSAMPLING * ENGINE_BLOCK],
    sub_high: [f32; OVERSAMPLING * ENGINE_BLOCK],
    carrier_down: Oversampler,
    sub_down: Oversampler,
    tables: &'static Tables,
}

impl FmEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            carrier_phase: 0.0,
            modulator_phase: 0.0,
            sub_phase: 0.0,
            carrier_frequency: BlockRamp::new(0.0),
            modulator_frequency: BlockRamp::new(0.0),
            amount: BlockRamp::new(0.0),
            feedback: BlockRamp::new(0.0),
            previous_sample: 0.0,
            carrier_high: [0.0; OVERSAMPLING * ENGINE_BLOCK],
            sub_high: [0.0; OVERSAMPLING * ENGINE_BLOCK],
            carrier_down: Oversampler::new(Factor::Four, sample_rate),
            sub_down: Oversampler::new(Factor::Four, sample_rate),
            tables: tables(),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        // Frequencies at the oversampled rate.
        let note = p.note - OVERSAMPLING_SEMITONES;
        let ratio = ratio_interval(&self.tables.fm_ratios, p.harmonics);
        let modulator_note = note + ratio;
        let modulator_frequency = p.note_to_frequency(modulator_note).clamp(0.0, 0.5);

        // Lower the most modulation high notes can take, to keep the
        // sidebands from aliasing.
        let hf_taming = (1.0 - (modulator_note - 72.0) * 0.025).clamp(0.0, 1.0);
        let hf_taming = hf_taming * hf_taming;

        let carriers = self
            .carrier_frequency
            .steps(p.note_to_frequency(note), size);
        let modulators = self.modulator_frequency.steps(modulator_frequency, size);
        let amounts = self
            .amount
            .steps(2.0 * p.timbre * p.timbre * hf_taming, size);
        let feedbacks = self.feedback.steps(2.0 * p.morph - 1.0, size);

        let high = self
            .carrier_high
            .chunks_exact_mut(OVERSAMPLING)
            .zip(self.sub_high.chunks_exact_mut(OVERSAMPLING));
        for ((carrier_out, sub_out), ((carrier_f, modulator_f), (amount, feedback))) in
            high.zip(carriers.zip(modulators).zip(amounts.zip(feedbacks)))
        {
            let phase_feedback = if feedback < 0.0 {
                0.5 * feedback * feedback
            } else {
                0.0
            };
            let modulator_feedback = if feedback > 0.0 {
                0.25 * feedback * feedback
            } else {
                0.0
            };
            for (c, s) in carrier_out.iter_mut().zip(sub_out.iter_mut()) {
                self.modulator_phase = wrap(
                    self.modulator_phase
                        + modulator_f * (1.0 + self.previous_sample * phase_feedback),
                );
                self.carrier_phase = wrap(self.carrier_phase + carrier_f);
                self.sub_phase = wrap(self.sub_phase + carrier_f * 0.5);
                let modulator =
                    sine(self.modulator_phase + modulator_feedback * self.previous_sample);
                let carrier = sine(self.carrier_phase + amount * modulator);
                *s = sine(self.sub_phase + amount * carrier * 0.25);
                self.previous_sample += 0.05 * (carrier - self.previous_sample);
                *c = carrier;
            }
        }
        let high = OVERSAMPLING * size;
        self.carrier_down
            .downsample(&self.carrier_high[..high], out);
        self.sub_down.downsample(&self.sub_high[..high], aux);
    }
}

#[inline]
fn wrap(phase: f32) -> f32 {
    phase - phase.floor()
}

/// The modulator's interval above the carrier for `harmonics`, in
/// semitones, read from the ratio table.
fn ratio_interval(table: &[f32; RATIO_STEPS + 2], harmonics: f32) -> f32 {
    let position = harmonics.clamp(0.0, 1.0) * RATIO_STEPS as f32;
    let i = position as usize;
    let f = position - i as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonics_reaches_unison_and_the_extremes() {
        let table = &tables().fm_ratios;
        assert!((ratio_interval(table, 0.0) + 12.0).abs() < 1e-4);
        assert_eq!(ratio_interval(table, 0.5), 12.0);
        assert!((ratio_interval(table, 1.0) - 36.0).abs() < 1e-4);
        // Some setting of harmonics lands exactly on unison.
        assert!((0..=128).any(|i| ratio_interval(table, i as f32 / 128.0) == 0.0));
    }
}
