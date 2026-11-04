//! Ported from Mutable Instruments' `plaits/dsp/engine/waveshaping_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.
//!
//! A slope (triangle to saw) through a waveshaper and a wavefolder.
//! `harmonics` picks the waveshaper curve, from the middle (linear)
//! outward to rounder or spikier ones; `timbre` drives the wavefolder;
//! `morph` skews the slope from a triangle toward a saw. `aux` is a
//! triangle-driven sine folded through a model of an analog wavefolder.

use super::tables::{hermite, tables, Tables, SHAPER_SIZE};
use crate::dsp::{sine, BlockRamp};
use crate::modules::macro_oscillator::engine::EngineParameters;
use crate::modules::macro_oscillator::oscillators::SlopeOscillator;

pub(in crate::modules::macro_oscillator) struct WaveshapingEngine {
    slope: SlopeOscillator,
    triangle: SlopeOscillator,
    shape: BlockRamp,
    wavefolder_gain: BlockRamp,
    overtone_gain: BlockRamp,
    tables: &'static Tables,
}

impl WaveshapingEngine {
    pub fn new() -> Self {
        Self {
            slope: SlopeOscillator::new(),
            triangle: SlopeOscillator::new(),
            shape: BlockRamp::new(0.0),
            wavefolder_gain: BlockRamp::new(0.0),
            overtone_gain: BlockRamp::new(0.0),
            tables: tables(),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        let f0 = p.note_to_frequency(p.note);
        let pw = p.morph * 0.45 + 0.5;

        // Start from a band-limited slope.
        self.slope.render(f0, pw, out);
        self.triangle.render(f0, 0.5, aux);

        // Estimate how rich the spectrum already is, and narrow the
        // waveshaper's range as it nears Nyquist so the shaping can't alias.
        let slope = 3.0 + (p.morph - 0.5).abs() * 5.0;
        let shape_amount = (p.harmonics - 0.5).abs() * 2.0;
        let shape_attenuation = tame(f0, slope, 16.0);
        let gain_attenuation = tame(
            f0,
            slope * (3.0 + shape_amount * shape_attenuation * 5.0),
            12.0,
        );
        let overtone = p.timbre * (2.0 - p.timbre);

        let shapes = self
            .shape
            .steps(0.5 + (p.harmonics - 0.5) * shape_attenuation, size);
        let gains = self
            .wavefolder_gain
            .steps(0.03 + 0.46 * p.timbre * gain_attenuation, size);
        let overtones = self.overtone_gain.steps(overtone * (2.0 - overtone), size);

        let t = self.tables;
        for ((o, a), (shape, (gain, overtone))) in out
            .iter_mut()
            .zip(aux.iter_mut())
            .zip(shapes.zip(gains.zip(overtones)))
        {
            let shape = shape * 3.9999;
            let shape_index = (shape as usize).min(3);
            let shape_fraction = shape - shape_index as f32;
            let (first, second) = (&t.shapers[shape_index], &t.shapers[shape_index + 1]);

            // The slope may overshoot ±1 a touch at its corners: hold the
            // read inside the curve rather than wrapping to its far end.
            let position = (127.0 * *o + 128.0).clamp(0.0, (SHAPER_SIZE - 2) as f32);
            let i = position as usize;
            let f = position - i as f32;
            let x = first[i] + (first[i + 1] - first[i]) * f;
            let y = second[i] + (second[i + 1] - second[i]) * f;
            let mix = x + (y - x) * shape_fraction;

            let index = mix * gain + 0.5;
            let fold = hermite(&t.fold, index);
            let fold_2 = -hermite(&t.fold_2, index);
            let sine_wave = sine(*a * 0.25 + 0.5);
            *o = fold;
            *a = sine_wave + (fold_2 - sine_wave) * overtone;
        }
    }
}

/// How much of a waveshaping effect to keep for a signal at `f0` whose
/// spectrum reaches `harmonics` times higher, for a shaper that multiplies
/// frequencies by up to `order`: full while that stays below Nyquist,
/// easing to none as it would reach it.
fn tame(f0: f32, harmonics: f32, order: f32) -> f32 {
    let f0 = f0 * harmonics;
    let max_f = 0.5 / order;
    let amount = (1.0 - (f0 - max_f) / (0.5 - max_f)).clamp(0.0, 1.0);
    amount * amount * amount
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tame_keeps_low_notes_and_mutes_near_nyquist() {
        assert_eq!(tame(0.001, 3.0, 16.0), 1.0);
        assert_eq!(tame(0.2, 3.0, 16.0), 0.0);
        let middle = tame(0.05, 3.0, 16.0);
        assert!(middle > 0.0 && middle < 1.0, "{middle}");
    }
}
