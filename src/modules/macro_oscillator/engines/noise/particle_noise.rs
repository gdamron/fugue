//! Ported from Mutable Instruments' `plaits/dsp/engine/particle_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! Particle noise: random impulses, each ringing a band-pass near the
//! pitch, like rain or gravel. `harmonics` spreads the bands' pitches;
//! `timbre` is the density, from sparse drops to a hiss; `morph` below
//! 0.5 smears the particles in a diffuser, above it narrows the bands
//! toward pitched pings. `audio` is the ringing, `aux` the bare impulses.

use super::super::filters::{flush, tan_dirty, ModeFilter, Rate};
use super::diffuser::Diffuser;
use super::particle::{Burst, Particle};
use crate::modules::macro_oscillator::engine::{semitones_to_ratio, EngineParameters};

const PARTICLES: usize = 6;

pub(in crate::modules::macro_oscillator) struct ParticleNoiseEngine {
    rate: Rate,
    particles: [Particle; PARTICLES],
    post_filter: ModeFilter,
    diffuser: Diffuser,
}

impl ParticleNoiseEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: Rate::new(sample_rate),
            particles: std::array::from_fn(|i| Particle::new(sample_rate, 0x5041 + i as u64)),
            post_filter: ModeFilter::default(),
            diffuser: Diffuser::new(sample_rate),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let to_48k = self.rate.at_48k(1.0);
        let f0 = p.note_to_frequency(p.note);
        // The density is set at 48 kHz, as particles a second.
        let density_sqrt = self
            .rate
            .at_48k(p.note_to_frequency(60.0 + p.timbre * p.timbre * 72.0));
        let density = density_sqrt * density_sqrt / PARTICLES as f32;
        let q_sqrt = semitones_to_ratio(if p.morph >= 0.5 {
            (p.morph - 0.5) * 120.0
        } else {
            0.0
        });
        let raw_diffusion_sqrt = 2.0 * (p.morph - 0.5).abs();
        let raw_diffusion = raw_diffusion_sqrt * raw_diffusion_sqrt;
        let diffusion = if p.morph < 0.5 { raw_diffusion } else { 0.0 };
        let burst = Burst {
            sync: p.trigger,
            sync_share: 1.0 / PARTICLES as f32,
            density: density / to_48k,
            frequency: f0,
            spread: 48.0 * p.harmonics * p.harmonics,
            q: 0.5 + q_sqrt * q_sqrt,
            // A one-sample impulse carries less at a higher rate.
            level: 0.5 / density.sqrt().sqrt() * to_48k,
        };

        out.fill(0.0);
        aux.fill(0.0);
        for particle in &mut self.particles {
            particle.render(&burst, out, aux);
        }
        self.post_filter.set_g_q(tan_dirty(f0.min(0.49)), 0.5);
        for o in out.iter_mut() {
            *o = self.post_filter.process(*o).lowpass;
        }
        self.diffuser
            .process(0.8 * diffusion * diffusion, 0.5 * diffusion + 0.25, out);
        // The ringing bands and the smear fade toward subnormal numbers.
        for x in out.iter_mut().chain(aux.iter_mut()) {
            *x = flush(*x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::filters::testing::{params, peak, play};
    use super::*;
    use crate::modules::macro_oscillator::engine::ENGINE_BLOCK;

    fn render(rate: u32, p: EngineParameters, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let mut engine = ParticleNoiseEngine::new(rate);
        let p = EngineParameters {
            sample_period: 1.0 / rate as f32,
            ..p
        };
        let blocks = (seconds * rate as f32) as usize / ENGINE_BLOCK;
        play(|p, o, a| engine.render(p, o, a), &p, blocks)
    }

    fn db(x: &[f32]) -> f32 {
        10.0 * (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).log10()
    }

    #[test]
    fn it_sounds_steadily() {
        let (out, aux) = render(48_000, params(60.0, false, true), 2.0);
        for x in [&out[48_000..72_000], &out[72_000..]] {
            assert!(peak(x) > 0.05, "{}", peak(x));
        }
        assert!(peak(&aux) > 0.1);
    }

    /// More `timbre`, more particles: more of the samples carry one.
    #[test]
    fn timbre_thickens_the_particles() {
        let impulses = |timbre: f32| {
            let (_, aux) = render(
                48_000,
                EngineParameters {
                    timbre,
                    ..params(60.0, false, true)
                },
                1.0,
            );
            aux.iter().filter(|s| **s != 0.0).count()
        };
        let (sparse, dense) = (impulses(0.2), impulses(0.8));
        assert!(dense > 10 * sparse.max(1), "{sparse} {dense}");
    }

    /// The particles come as often, and ring as loud, at any rate. (The
    /// bare impulses on `aux` are one sample wide, so like any click they
    /// carry less at a higher rate.)
    #[test]
    fn it_sounds_alike_at_any_rate() {
        for (timbre, morph) in [(0.85, 0.5), (0.9, 0.8), (0.9, 0.2)] {
            let p = EngineParameters {
                timbre,
                morph,
                ..params(60.0, false, true)
            };
            let measure = |rate: u32| {
                let (out, aux) = render(rate, p, 3.0);
                let count = aux.iter().filter(|s| **s != 0.0).count();
                (db(&out[rate as usize..]), count)
            };
            let (want, want_count) = measure(48_000);
            for rate in [44_100, 96_000] {
                let (got, count) = measure(rate);
                assert!(
                    (got - want).abs() < 1.0,
                    "{timbre}/{morph} at {rate} Hz: {got} dB against {want}"
                );
                let ratio = count as f32 / want_count as f32;
                assert!(
                    (ratio - 1.0).abs() < 0.2,
                    "{timbre}/{morph} at {rate} Hz: {count} against {want_count}"
                );
            }
        }
    }
}
