//! Ported from Mutable Instruments' `plaits/dsp/engine/virtual_analog_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block processing.
//! See THIRD_PARTY_NOTICES.md.
//!
//! Two analog-style oscillators. `harmonics` detunes the second one, from
//! unison through fifths and octaves; `timbre` sets the first one's pulse
//! width and sync; `morph` turns the second one from a notched saw into a
//! triangle. `aux` is a pair of hard-synced variable waveforms.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};
use crate::modules::macro_oscillator::oscillators::{
    VariableSawOscillator, VariableShapeOscillator,
};

/// The detune stops `harmonics` passes through, in semitones (slightly
/// wide, so the stacked intervals beat a little).
const INTERVALS: [f32; 5] = [0.0, 7.01, 12.01, 19.01, 24.01];

pub(in crate::modules::macro_oscillator) struct VirtualAnalogEngine {
    primary: VariableShapeOscillator,
    auxiliary: VariableShapeOscillator,
    sync: VariableShapeOscillator,
    variable_saw: VariableSawOscillator,
    square_level: BlockRamp,
    saw_level: BlockRamp,
    temp: [f32; ENGINE_BLOCK],
}

impl VirtualAnalogEngine {
    pub fn new() -> Self {
        Self {
            primary: VariableShapeOscillator::new(),
            auxiliary: VariableShapeOscillator::new().with_master_phase(0.25),
            sync: VariableShapeOscillator::new(),
            variable_saw: VariableSawOscillator::new(),
            square_level: BlockRamp::new(0.0),
            saw_level: BlockRamp::new(0.0),
            temp: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        let sync_amount = p.timbre * p.timbre;
        let auxiliary_detune = detuning(p.harmonics);
        let primary_f = p.note_to_frequency(p.note);
        let auxiliary_f = p.note_to_frequency(p.note + auxiliary_detune);
        let primary_sync_f = p.note_to_frequency(p.note + sync_amount * 48.0);
        let auxiliary_sync_f = p.note_to_frequency(p.note + auxiliary_detune + sync_amount * 48.0);

        let shape = (p.morph * 1.5).clamp(0.0, 1.0);
        let pw = (0.5 + (p.morph - 0.66) * 1.46).clamp(0.5, 0.995);

        // AUX: a pair of synced variable waveforms ("monster sync").
        self.primary
            .render(true, primary_f, primary_sync_f, pw, shape, out);
        self.auxiliary
            .render(true, auxiliary_f, auxiliary_sync_f, pw, shape, aux);
        for (a, o) in aux.iter_mut().zip(out.iter()) {
            *a = (*a - *o) * 0.5;
        }

        // OUT: a variable square plus a variable saw.
        let square_pw = (1.3 * p.timbre - 0.15).clamp(0.005, 0.5);
        let square_sync_ratio = if p.timbre < 0.5 {
            0.0
        } else {
            (p.timbre - 0.5) * (p.timbre - 0.5) * 4.0 * 48.0
        };
        let square_level = (p.timbre * 8.0).min(1.0);

        let saw_pw = if p.morph < 0.5 {
            p.morph + 0.5
        } else {
            1.0 - (p.morph - 0.5) * 2.0
        };
        let saw_pw = (saw_pw * 1.1).clamp(0.005, 1.0);
        let saw_shape = (10.0 - 21.0 * p.morph).clamp(0.0, 1.0);
        let saw_level = (8.0 * (1.0 - p.morph)).clamp(0.02, 1.0);

        let square_sync_f = p.note_to_frequency(p.note + square_sync_ratio);
        let temp = &mut self.temp[..size];
        self.sync
            .render(true, primary_f, square_sync_f, square_pw, 1.0, temp);
        self.variable_saw
            .render(auxiliary_f, saw_pw, saw_shape, out);

        let norm = 1.0 / square_level.max(saw_level);
        let square_ramp = self.square_level.steps(square_level * 0.3 * norm, size);
        let saw_ramp = self.saw_level.steps(saw_level * 0.5 * norm, size);
        for ((o, t), (saw, square)) in out
            .iter_mut()
            .zip(temp.iter())
            .zip(saw_ramp.zip(square_ramp))
        {
            *o = *o * saw + square * *t;
        }
    }
}

/// The second oscillator's detune for `harmonics`, in semitones: unison
/// at the middle, widening through fifths and octaves to two octaves
/// either way.
fn detuning(harmonics: f32) -> f32 {
    let detune = (2.05 * harmonics - 1.025).clamp(-1.0, 1.0);
    let sign = if detune < 0.0 { -1.0 } else { 1.0 };
    let detune = detune * sign * 3.9999;
    let integral = detune as usize;
    let fractional = detune - integral as f32;
    let a = INTERVALS[integral];
    let b = INTERVALS[integral + 1];
    (a + (b - a) * squash(squash(fractional))) * sign
}

#[inline]
fn squash(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonics_detunes_from_unison_to_two_octaves() {
        assert_eq!(detuning(0.5), 0.0);
        assert!((detuning(1.0) - 24.01).abs() < 0.01);
        assert!((detuning(0.0) + 24.01).abs() < 0.01);
    }
}
