//! Ported from Mutable Instruments' `plaits/dsp/engine/snare_drum_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! The snare drum. `harmonics` balances the shell against the snares;
//! `timbre` brings in the shell's upper modes (on `aux`, the pitch blip);
//! `morph` is the decay. `audio` is the analog-style drum, `aux` the
//! synthetic one.

use super::analog_snare::{AnalogSnareDrum, AnalogSnareSettings};
use super::synthetic_snare::{SyntheticSnareDrum, SyntheticSnareSettings};
use crate::modules::macro_oscillator::engine::EngineParameters;

pub(in crate::modules::macro_oscillator) struct SnareDrumEngine {
    analog: AnalogSnareDrum,
    synthetic: SyntheticSnareDrum,
}

impl SnareDrumEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            analog: AnalogSnareDrum::new(sample_rate),
            synthetic: SyntheticSnareDrum::new(sample_rate),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let f0 = p.note_to_frequency(p.note);
        self.analog.render(
            &AnalogSnareSettings {
                sustain: p.untriggered,
                trigger: p.trigger,
                accent: p.accent,
                f0,
                tone: p.timbre,
                decay: p.morph,
                snappy: p.harmonics,
            },
            out,
        );
        self.synthetic.render(
            &SyntheticSnareSettings {
                sustain: p.untriggered,
                trigger: p.trigger,
                accent: p.accent,
                f0,
                fm_amount: p.timbre,
                decay: p.morph,
                snappy: p.harmonics,
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

    fn engine() -> SnareDrumEngine {
        SnareDrumEngine::new(RATE)
    }

    #[test]
    fn it_waits_for_a_strike_and_then_rings_out() {
        let mut drum = engine();
        let (out, aux) = play(
            |p, o, a| drum.render(p, o, a),
            &params(50.0, false, false),
            400,
        );
        assert!(peak(&out) + peak(&aux) < 1e-9);
        let (out, aux) = strike(
            |p, o, a| drum.render(p, o, a),
            params(50.0, false, false),
            4.0,
        );
        for x in [&out, &aux] {
            assert!(peak(&x[..4_800]) > 0.1, "struck at {}", peak(&x[..4_800]));
            let tail = peak(&x[x.len() - 4_800..]);
            assert!(tail < 10f32.powf(-90.0 / 20.0), "rings at {tail}");
        }
    }

    /// More `harmonics`, more snares: the sound gets noisier, so it
    /// crosses zero more often.
    #[test]
    fn harmonics_brings_in_the_snares() {
        let crossings = |harmonics: f32| {
            let mut drum = engine();
            let p = EngineParameters {
                harmonics,
                ..params(50.0, false, false)
            };
            let (out, aux) = strike(|p, o, a| drum.render(p, o, a), p, 0.1);
            [out, aux].map(|x| {
                x.windows(2)
                    .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                    .count()
            })
        };
        let (shell, snares) = (crossings(0.1), crossings(0.9));
        assert!(snares[0] > 2 * shell[0], "{shell:?} {snares:?}");
        assert!(snares[1] > 2 * shell[1], "{shell:?} {snares:?}");
    }

    #[test]
    fn it_sounds_alike_at_any_rate() {
        assert_rate_independent(|rate| {
            loudness_over_time(rate, 50.0, SnareDrumEngine::new, |d, p, o, a| {
                d.render(p, o, a)
            })
        });
    }

    #[test]
    fn un_triggered_it_sounds_steadily() {
        let mut drum = engine();
        let p = params(50.0, false, true);
        play(|p, o, a| drum.render(p, o, a), &p, 400);
        let (out, aux) = play(|p, o, a| drum.render(p, o, a), &p, 4_000);
        for x in [&out, &aux] {
            let halves = [&x[..24_000], &x[24_000..]].map(peak);
            assert!(halves[0] > 0.05, "{halves:?}");
            assert!((halves[0] / halves[1] - 1.0).abs() < 0.5, "{halves:?}");
        }
    }
}
