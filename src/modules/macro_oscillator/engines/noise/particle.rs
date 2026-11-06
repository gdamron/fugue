//! Ported from Mutable Instruments' `plaits/dsp/noise/particle.h` (MIT,
//! Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.

use super::super::filters::{tan_dirty, ModeFilter, Rate};
use crate::dsp::Rng;
use crate::modules::macro_oscillator::engine::semitones_to_ratio;

/// One block's settings for every particle.
pub(super) struct Burst {
    pub sync: bool,
    /// Each particle's share of `aux` for the burst a sync fires, so the
    /// particles all firing at once sum to one impulse rather than clip.
    pub sync_share: f32,
    /// The chance of a particle each sample.
    pub density: f32,
    /// The pitch, in cycles per sample, and how many semitones each
    /// particle may stray from it.
    pub frequency: f32,
    pub spread: f32,
    pub q: f32,
    /// The particles' loudness held level whatever their density, judged
    /// at 48 kHz: the band-pass's gain is `1 / √(q · f₄₈ₖ)` times this.
    pub level: f32,
}

/// Random impulses, each ringing a band-pass tuned at random about the
/// pitch.
pub(super) struct Particle {
    rate: Rate,
    rng: Rng,
    pre_gain: f32,
    filter: ModeFilter,
}

impl Particle {
    pub fn new(sample_rate: u32, seed: u64) -> Self {
        Self {
            rate: Rate::new(sample_rate),
            rng: Rng::new(seed),
            pre_gain: 0.0,
            filter: ModeFilter::default(),
        }
    }

    /// Adds this particle's ringing to `out` and its bare impulses to
    /// `aux`.
    pub fn render(&mut self, b: &Burst, out: &mut [f32], aux: &mut [f32]) {
        let gain = 1.0 / b.density;
        let mut u = if b.sync { b.density } else { self.rng.unit() };
        let mut retune = true;
        let mut share = if b.sync { b.sync_share } else { 1.0 };
        for (o, a) in out.iter_mut().zip(aux.iter_mut()) {
            let mut s = 0.0;
            if u <= b.density {
                s = u * gain;
                // Each block keeps the band the first particle found.
                if retune {
                    retune = false;
                    let detune = 2.0 * self.rng.unit() - 1.0;
                    let f = (semitones_to_ratio(b.spread * detune) * b.frequency).min(0.25);
                    self.pre_gain = b.level / (b.q * self.rate.at_48k(f)).sqrt();
                    self.filter.set_g_q(tan_dirty(f), b.q);
                }
            }
            *a += s * share;
            share = 1.0;
            *o += self.filter.process(self.pre_gain * s).bandpass;
            u = self.rng.unit();
        }
    }
}
