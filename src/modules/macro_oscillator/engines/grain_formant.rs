//! Ported from Mutable Instruments' `plaits/dsp/engine/grain_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.
//!
//! Formants from windowed sine grains. `timbre` sets the formant
//! frequency; `harmonics` sets the second formant's ratio to the first
//! (two octaves down to two up) and, below the middle, bleeds the bare
//! grain window in; `morph` shapes the window, from a short burst to a
//! slow swell. `aux` is a sync-swept resonance whose cutoff `timbre`
//! sets and whose character `harmonics` and `morph` select.

use crate::dsp::OnePole;
use crate::modules::macro_oscillator::engine::{
    semitones_to_ratio, EngineParameters, ENGINE_BLOCK,
};
use crate::modules::macro_oscillator::oscillators::{GrainletOscillator, ZOscillator};

/// The DC blockers' cutoff, as a fraction of the pitch.
const DC_CUTOFF: f32 = 0.3;

pub(in crate::modules::macro_oscillator) struct GrainFormantEngine {
    grainlets: [GrainletOscillator; 2],
    z: ZOscillator,
    dc_blockers: [OnePole; 2],
    second: [f32; ENGINE_BLOCK],
}

impl GrainFormantEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            grainlets: [GrainletOscillator::new(), GrainletOscillator::new()],
            z: ZOscillator::new(),
            dc_blockers: [
                OnePole::new(sample_rate, 20.0),
                OnePole::new(sample_rate, 20.0),
            ],
            second: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        let f0 = p.note_to_frequency(p.note);
        let f1 = p.note_to_frequency(24.0 + 84.0 * p.timbre);
        let ratio = semitones_to_ratio(-24.0 + 48.0 * p.harmonics);
        let bleed = if p.harmonics < 0.5 {
            1.0 - 2.0 * p.harmonics
        } else {
            0.0
        };
        let bleed = bleed * (2.0 - bleed);
        let shape = 0.33 + (p.morph - 0.33) * (1.0 - f0 * 24.0).max(0.0);

        let second = &mut self.second[..size];
        self.grainlets[0].render(f0, f1, shape, bleed, out);
        self.grainlets[1].render(f0, f1 * ratio, shape, bleed, second);
        let dc_cutoff = DC_CUTOFF * f0 / p.sample_period;
        self.dc_blockers[0].set_cutoff(dc_cutoff);
        for (o, s) in out.iter_mut().zip(second.iter()) {
            *o = self.dc_blockers[0].highpass(*o + *s);
        }

        let cutoff = p.note_to_frequency(p.note + 96.0 * p.timbre);
        self.z.render(f0, cutoff, p.morph, p.harmonics, aux);
        self.dc_blockers[1].set_cutoff(dc_cutoff);
        for a in aux.iter_mut() {
            *a = self.dc_blockers[1].highpass(*a);
        }
    }
}
