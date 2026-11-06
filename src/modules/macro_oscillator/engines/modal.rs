//! Ported from Mutable Instruments' `plaits/dsp/engine/modal_engine.cc` and
//! `plaits/dsp/physical_modelling/modal_voice.cc` (MIT, Copyright 2016
//! Emilie Gillet), adapted to Fugue: the body is the resonator's bank of
//! modes (ported from the same family), struck with Plaits' exciter. See
//! THIRD_PARTY_NOTICES.md.
//!
//! A struck body: a bank of tuned modes rung by a mallet. `harmonics` sets
//! the structure, from crowded partials through harmonic ones to bells and
//! metal plates; `timbre` is the mallet's brightness, `morph` how long the
//! body rings. Un-triggered, it is rubbed by a stream of dust. `aux` is the
//! mallet alone.

use super::dust::dust;
use crate::dsp::physical::{ModalSettings, ModalVoice};
use crate::dsp::{exp2, Rng, Svf};
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

/// How many modes the body rings, as in the reference.
const MODES: usize = 24;
/// Where the body is heard from, as a fraction of its length.
const PICKUP: f32 = 0.015;
/// How far `harmonics` glides toward its target per block, at 48 kHz.
const STRUCTURE_SMOOTHING: f32 = 0.01;

pub(in crate::modules::macro_oscillator) struct ModalEngine {
    sample_rate: f32,
    body: ModalVoice,
    mallet: Svf,
    structure: Option<f32>,
    rng: Rng,
    excitation: [f32; ENGINE_BLOCK],
    even: [f32; ENGINE_BLOCK],
}

impl ModalEngine {
    pub fn new(sample_rate: u32) -> Self {
        let mut body = ModalVoice::new(sample_rate);
        body.set_resolution(MODES);
        Self {
            sample_rate: sample_rate.max(1) as f32,
            body,
            mallet: Svf::new(sample_rate, 1_000.0, 1.5),
            structure: None,
            rng: Rng::new(0x30da1),
            excitation: [0.0; ENGINE_BLOCK],
            even: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        let f0 = p.note_to_frequency(p.note).min(0.49);
        let smoothing = (STRUCTURE_SMOOTHING * 48_000.0 * p.sample_period).min(1.0);
        let structure = self.structure.get_or_insert(p.harmonics);
        *structure += (p.harmonics - *structure) * smoothing;

        let density = p.timbre * p.timbre;
        let brightness = p.timbre + 0.25 * p.accent * (1.0 - p.timbre);
        let damping = p.morph + 0.25 * p.accent * (1.0 - p.morph);
        let sustain = p.untriggered;
        let (range, f, q) = if sustain {
            (36.0, 4.0 * f0, 0.7)
        } else {
            (60.0, 2.0 * f0, 1.5)
        };
        let cutoff = (f * exp2((brightness * (2.0 - brightness) - 0.5) * range / 12.0)).min(0.499);
        self.mallet.set(cutoff * self.sample_rate, q);

        let excitation = &mut self.excitation[..size];
        if sustain {
            let dust_f = 0.000_05 + 0.999_95 * density * density;
            for x in excitation.iter_mut() {
                *x = dust(&mut self.rng, dust_f) * (4.0 - dust_f * 3.0) * p.accent;
            }
        } else {
            excitation.fill(0.0);
            if p.trigger {
                let attenuation = 1.0 - damping * 0.5;
                let amplitude = (0.12 + 0.08 * p.accent) * attenuation;
                excitation[0] = amplitude * exp2(cutoff * cutoff * 2.0) / cutoff;
            }
        }
        for (x, a) in excitation.iter_mut().zip(aux.iter_mut()) {
            *x = self.mallet.process(*x).lowpass;
            *a = *x;
        }

        out.fill(0.0);
        let even = &mut self.even[..size];
        even.fill(0.0);
        let settings = ModalSettings {
            frequency: f0,
            structure: *structure,
            brightness,
            damping,
            position: PICKUP,
        };
        self.body.process(&settings, excitation, out, even);
        for (o, e) in out.iter_mut().zip(even.iter()) {
            *o += e;
        }
    }
}
