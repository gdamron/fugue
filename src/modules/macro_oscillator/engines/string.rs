//! Ported from Mutable Instruments' `plaits/dsp/engine/string_engine.cc` and
//! `plaits/dsp/physical_modelling/string_voice.cc` (MIT, Copyright 2016
//! Emilie Gillet), adapted to Fugue: the string itself is the resonator's
//! (ported from the same family), kept in tune when stiff, and a string
//! that has fallen silent stops running until it is plucked again. See
//! THIRD_PARTY_NOTICES.md.
//!
//! An inharmonic string, plucked by the trigger. `harmonics` sets its
//! structure: below the middle a curved bridge makes it buzz and sitar-like,
//! above it the string stiffens toward a piano wire and then to wobbling,
//! dispersive metal. `timbre` is brightness, `morph` how long it rings.
//! Three strings take turns, so a new note lets the last one ring on.
//! Un-triggered, the active string is bowed by a stream of dust. `aux` is
//! the excitation alone.

use super::dust::dust;
use crate::dsp::physical::{KarplusString, StringSettings};
use crate::dsp::{exp2, Rng, Svf};
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

/// How many strings take turns.
const STRINGS: usize = 3;
/// A string whose loop holds nothing louder than this has fallen silent.
const SILENT: f32 = 1.0e-7;

pub(in crate::modules::macro_oscillator) struct StringEngine {
    sample_rate: f32,
    strings: [KarplusString; STRINGS],
    /// Each string's excitation filter.
    filters: [Svf; STRINGS],
    /// How many frames of noise burst each string has left.
    noise_left: [usize; STRINGS],
    /// Whether each string is ringing (else it is skipped).
    ringing: [bool; STRINGS],
    active: usize,
    rng: Rng,
    excitation: [f32; ENGINE_BLOCK],
    pickup: [f32; ENGINE_BLOCK],
}

impl StringEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f32,
            strings: std::array::from_fn(|i| {
                KarplusString::new(sample_rate, true, 0x5717 + i as u64)
                    .with_tuned_stiffness()
                    .with_dc_blocked_loop()
            }),
            filters: std::array::from_fn(|_| Svf::new(sample_rate, 1_000.0, 0.5)),
            noise_left: [0; STRINGS],
            ringing: [false; STRINGS],
            active: STRINGS - 1,
            rng: Rng::new(0x5717),
            excitation: [0.0; ENGINE_BLOCK],
            pickup: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        out.fill(0.0);
        aux.fill(0.0);
        if p.trigger {
            self.active = (self.active + 1) % STRINGS;
        }
        let f0 = p.note_to_frequency(p.note).min(0.49);
        self.strings[self.active].glide_to(f0, 1.0);

        // How far the string's excitation and loss lean bright, and how long
        // it rings; a harder note is brighter and rings longer.
        let brightness = p.timbre * p.timbre;
        let density = brightness * brightness;
        let brightness = brightness + 0.25 * p.accent * (1.0 - brightness);
        let damping = p.morph + 0.25 * p.accent * (1.0 - p.morph);
        let structure = p.harmonics;
        let dispersion = if structure < 0.24 {
            (structure - 0.24) * 4.166
        } else if structure > 0.26 {
            (structure - 0.26) * 1.351_35
        } else {
            0.0
        };
        let settings = StringSettings {
            dispersion,
            // The string squares its brightness itself.
            brightness: brightness.sqrt(),
            damping,
            position: 0.5,
        };

        for i in 0..STRINGS {
            let active = i == self.active;
            let sustain = active && p.untriggered;
            let struck = active && p.trigger;
            let excitation = &mut self.excitation[..size];
            if struck || sustain {
                let f = 4.0 * f0;
                let cutoff = (f * exp2((brightness * (2.0 - brightness) - 0.5) * 6.0)).min(0.499);
                let q = if sustain { 1.0 } else { 0.5 };
                if struck {
                    self.noise_left[i] = (1.0 / f0) as usize;
                }
                self.filters[i].set(cutoff * self.sample_rate, q);
            }
            if sustain {
                let dust_f = 0.000_05 + 0.999_95 * density * density;
                for x in excitation.iter_mut() {
                    *x = dust(&mut self.rng, dust_f) * (8.0 - dust_f * 6.0) * p.accent;
                }
            } else {
                let burst = self.noise_left[i].min(size);
                self.noise_left[i] -= burst;
                for (k, x) in excitation.iter_mut().enumerate() {
                    *x = if k < burst { self.rng.bipolar() } else { 0.0 };
                }
            }
            let excited = sustain || struck || self.noise_left[i] > 0;
            if !(self.ringing[i] || excited || excitation.iter().any(|x| *x != 0.0)) {
                continue;
            }
            for (x, a) in excitation.iter_mut().zip(aux.iter_mut()) {
                *x = self.filters[i].process(*x).lowpass;
                *a += *x;
            }
            let pickup = &mut self.pickup[..size];
            let level = self.strings[i].process(&settings, excitation, out, pickup);
            self.ringing[i] = level > SILENT || excited;
            if !self.ringing[i] {
                self.strings[i].reset();
                self.filters[i].reset();
            }
        }
    }
}
