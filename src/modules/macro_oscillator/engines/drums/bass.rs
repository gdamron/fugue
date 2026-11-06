//! Ported from Mutable Instruments' `plaits/dsp/engine/bass_drum_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! The bass drum. `harmonics` adds punch: first the attack's pitch blip,
//! then the drum bending its own pitch, then overdrive. `timbre` is the
//! tone, from a dull thud to a clicky one; `morph` is the decay. `audio`
//! is the analog-style drum, `aux` the synthetic one.

use super::analog_bass::{AnalogBassDrum, AnalogBassSettings};
use super::filters::Rate;
use super::overdrive::Overdrive;
use super::synthetic_bass::{SyntheticBassDrum, SyntheticBassSettings};
use crate::modules::macro_oscillator::engine::EngineParameters;

pub(in crate::modules::macro_oscillator) struct BassDrumEngine {
    rate: Rate,
    analog: AnalogBassDrum,
    overdrive: Overdrive,
    synthetic: SyntheticBassDrum,
}

impl BassDrumEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: Rate::new(sample_rate),
            analog: AnalogBassDrum::new(sample_rate),
            overdrive: Overdrive::default(),
            synthetic: SyntheticBassDrum::new(sample_rate),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let f0 = p.note_to_frequency(p.note);
        let h = p.harmonics;
        let drive = (h * 2.0 - 1.0).max(0.0) * (1.0 - 16.0 * self.rate.at_48k(f0)).max(0.0);
        let sustain = p.untriggered;

        self.analog.render(
            &AnalogBassSettings {
                sustain,
                trigger: p.trigger,
                accent: p.accent,
                f0,
                tone: p.timbre,
                decay: p.morph,
                attack_fm: (h * 4.0).min(1.0),
                self_fm: (h * 4.0 - 1.0).clamp(0.0, 1.0),
            },
            out,
        );
        self.overdrive.process(0.5 + 0.5 * drive, out);

        self.synthetic.render(
            &SyntheticBassSettings {
                sustain,
                trigger: p.trigger,
                accent: p.accent,
                f0,
                tone: p.timbre,
                decay: p.morph,
                dirtiness: if sustain {
                    h
                } else {
                    0.4 - 0.25 * p.morph * p.morph
                },
                fm_amount: (h * 2.0).min(1.0),
                fm_decay: (h * 2.0 - 1.0).max(0.0),
            },
            aux,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::filters::testing::{
        assert_rate_independent, loudness_over_time, params, peak, play, strike, RATE,
    };
    use super::*;

    fn engine() -> BassDrumEngine {
        BassDrumEngine::new(RATE)
    }

    #[test]
    fn it_waits_for_a_strike_and_then_rings_out() {
        let mut drum = engine();
        let (out, aux) = play(
            |p, o, a| drum.render(p, o, a),
            &params(36.0, false, false),
            400,
        );
        assert!(peak(&out) + peak(&aux) < 1e-9);
        let (out, aux) = strike(
            |p, o, a| drum.render(p, o, a),
            params(36.0, false, false),
            4.0,
        );
        for x in [&out, &aux] {
            assert!(peak(&x[..4_800]) > 0.1, "struck at {}", peak(&x[..4_800]));
            let tail = peak(&x[x.len() - 4_800..]);
            assert!(tail < 10f32.powf(-90.0 / 20.0), "rings at {tail}");
        }
    }

    #[test]
    fn un_triggered_it_hums_at_its_pitch() {
        let mut drum = engine();
        // Low harmonics, where the drum does not bend its own pitch.
        let p = EngineParameters {
            harmonics: 0.2,
            ..params(45.0, false, true)
        };
        play(|p, o, a| drum.render(p, o, a), &p, 400);
        let (out, aux) = play(|p, o, a| drum.render(p, o, a), &p, 4_000);
        for x in [&out, &aux] {
            assert!(peak(x) > 0.05);
            let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            // A (110 Hz) for one second.
            assert!((105..=115).contains(&crossings), "{crossings}");
        }
    }

    #[test]
    fn a_longer_decay_rings_longer() {
        let energy_after = |morph: f32| {
            let mut drum = engine();
            let p = EngineParameters {
                morph,
                ..params(36.0, false, false)
            };
            let (out, _) = strike(|p, o, a| drum.render(p, o, a), p, 1.0);
            out[24_000..].iter().map(|s| s * s).sum::<f32>()
        };
        assert!(energy_after(0.8) > 10.0 * energy_after(0.3));
    }

    #[test]
    fn it_sounds_alike_at_any_rate() {
        assert_rate_independent(|rate| {
            loudness_over_time(rate, 36.0, BassDrumEngine::new, |d, p, o, a| {
                d.render(p, o, a)
            })
        });
    }
}
