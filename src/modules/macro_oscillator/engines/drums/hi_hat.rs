//! Ported from Mutable Instruments' `plaits/dsp/engine/hi_hat_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! The hi-hat. `harmonics` roughens the metallic tone with noise;
//! `timbre` is the tone, the band the metal rings in; `morph` is the
//! decay, from closed to open. `audio` is the analog-style hat, `aux` a
//! ring-modulated one that chokes as it fades.

use super::hats::{HiHat, HiHatKind, HiHatSettings};
use crate::modules::macro_oscillator::engine::EngineParameters;

pub(in crate::modules::macro_oscillator) struct HiHatEngine {
    analog: HiHat,
    ringing: HiHat,
}

impl HiHatEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            analog: HiHat::new(sample_rate, HiHatKind::Analog),
            ringing: HiHat::new(sample_rate, HiHatKind::Ringing),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let settings = HiHatSettings {
            sustain: p.untriggered,
            trigger: p.trigger,
            accent: p.accent,
            f0: p.note_to_frequency(p.note),
            tone: p.timbre,
            decay: p.morph,
            noisiness: p.harmonics,
        };
        self.analog.render(&settings, out);
        self.ringing.render(&settings, aux);
    }
}

#[cfg(test)]
mod tests {
    use super::super::filters::testing::{
        assert_rate_independent, loudness_over_time, params, peak, play, strike, RATE,
    };
    use super::*;

    const NOTE: f32 = 60.0;

    fn engine() -> HiHatEngine {
        HiHatEngine::new(RATE)
    }

    #[test]
    fn it_waits_for_a_strike_and_then_rings_out() {
        let mut hat = engine();
        let (out, aux) = play(
            |p, o, a| hat.render(p, o, a),
            &params(NOTE, false, false),
            400,
        );
        assert!(peak(&out) + peak(&aux) < 1e-9);
        let (out, aux) = strike(
            |p, o, a| hat.render(p, o, a),
            params(NOTE, false, false),
            4.0,
        );
        for x in [&out, &aux] {
            assert!(peak(&x[..4_800]) > 0.1, "struck at {}", peak(&x[..4_800]));
            let tail = peak(&x[x.len() - 4_800..]);
            assert!(tail < 10f32.powf(-90.0 / 20.0), "rings at {tail}");
        }
    }

    /// A longer `morph` opens the hat: more of it is left after 0.2 s.
    #[test]
    fn morph_opens_the_hat() {
        let late = |morph: f32| {
            let mut hat = engine();
            let p = EngineParameters {
                morph,
                ..params(NOTE, false, false)
            };
            let (out, aux) = strike(|p, o, a| hat.render(p, o, a), p, 0.3);
            [peak(&out[9_600..]), peak(&aux[9_600..])]
        };
        let (closed, open) = (late(0.2), late(0.8));
        assert!(open[0] > 4.0 * closed[0], "{closed:?} {open:?}");
        assert!(open[1] > 4.0 * closed[1], "{closed:?} {open:?}");
    }

    /// A higher `timbre` moves the band up, so the hat crosses zero more.
    #[test]
    fn timbre_brightens_the_hat() {
        let crossings = |timbre: f32| {
            let mut hat = engine();
            let p = EngineParameters {
                timbre,
                ..params(NOTE, false, false)
            };
            let (out, aux) = strike(|p, o, a| hat.render(p, o, a), p, 0.05);
            [out, aux].map(|x| {
                x.windows(2)
                    .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                    .count()
            })
        };
        let (dark, bright) = (crossings(0.2), crossings(0.8));
        assert!(
            bright[0] > dark[0] && bright[1] > dark[1],
            "{dark:?} {bright:?}"
        );
    }

    #[test]
    fn it_sounds_alike_at_any_rate() {
        assert_rate_independent(|rate| {
            loudness_over_time(rate, NOTE, HiHatEngine::new, |h, p, o, a| h.render(p, o, a))
        });
    }

    #[test]
    fn un_triggered_it_sounds_steadily() {
        let mut hat = engine();
        let p = params(NOTE, false, true);
        play(|p, o, a| hat.render(p, o, a), &p, 400);
        let (out, aux) = play(|p, o, a| hat.render(p, o, a), &p, 4_000);
        for x in [&out, &aux] {
            let halves = [&x[..24_000], &x[24_000..]].map(peak);
            assert!(halves[0] > 0.05, "{halves:?}");
            assert!((halves[0] / halves[1] - 1.0).abs() < 0.5, "{halves:?}");
        }
    }
}
