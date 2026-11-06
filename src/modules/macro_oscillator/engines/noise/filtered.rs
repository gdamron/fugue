//! Ported from Mutable Instruments' `plaits/dsp/engine/noise_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! Filtered noise. `harmonics` sweeps the filter from low-pass through
//! band-pass to high-pass, and on `aux` sets a second band's distance
//! from the first, two octaves below to two above; `timbre` is the noise
//! clock, from grainy to smooth; `morph` the resonance. `audio` is the
//! swept filter, `aux` two band-passes.

use super::super::drums::filters::{tan_accurate, BlockGlide, ModeFilter, Rate};
use super::clocked::ClockedNoise;
use crate::modules::macro_oscillator::engine::{
    semitones_to_ratio, EngineParameters, ENGINE_BLOCK,
};

/// The highest band, in cycles per sample, short of where the cutoff's
/// tangent runs away.
const MAX_CUTOFF: f32 = 0.49;

pub(in crate::modules::macro_oscillator) struct FilteredNoiseEngine {
    rate: Rate,
    clocked: [ClockedNoise; 2],
    sweep: ModeFilter,
    bands: [ModeFilter; 2],
    f0: BlockGlide,
    f1: BlockGlide,
    q: BlockGlide,
    mode: BlockGlide,
    second: [f32; ENGINE_BLOCK],
}

impl FilteredNoiseEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: Rate::new(sample_rate),
            clocked: [
                ClockedNoise::new(sample_rate, 0x4e31),
                ClockedNoise::new(sample_rate, 0x4e32),
            ],
            sweep: ModeFilter::default(),
            bands: Default::default(),
            f0: BlockGlide::default(),
            f1: BlockGlide::default(),
            q: BlockGlide::default(),
            mode: BlockGlide::default(),
            second: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        let f0 = p.note_to_frequency(p.note).min(MAX_CUTOFF);
        let f1 = p
            .note_to_frequency(p.note + p.harmonics * 48.0 - 24.0)
            .min(MAX_CUTOFF);
        // Struck, the clock reaches two octaves lower, for sparser grains.
        let lowest = if p.untriggered { 0.0 } else { -24.0 };
        let clock_f = p.note_to_frequency(p.timbre * (128.0 - lowest) + lowest);
        let q = 0.5 * semitones_to_ratio(p.morph * 120.0);
        let second = &mut self.second[..size];
        self.clocked[0].render(p.trigger, clock_f, aux);
        self.clocked[1].render(p.trigger, clock_f * f1 / f0, second);

        self.f0.start(f0, size);
        self.f1.start(f1, size);
        self.q.start(q, size);
        self.mode.start(p.harmonics, size);
        for ((o, a), b) in out.iter_mut().zip(aux.iter_mut()).zip(second.iter()) {
            let (f0, f1, q, mode) = (
                self.f0.next(),
                self.f1.next(),
                self.q.next(),
                self.mode.next(),
            );
            // The band's loudness held level whatever its width, judged at
            // 48 kHz so the held noise is as loud at any rate.
            let gain = 1.0 / ((0.5 + q) * 40.0 * self.rate.at_48k(f0)).sqrt();
            let g0 = tan_accurate(f0);
            self.sweep.set_g_q(g0, q);
            self.bands[0].set_g_q(g0, q);
            self.bands[1].set_g_q(tan_accurate(f1), q);

            let (first, second) = (*a * gain, b * gain);
            let swept = self.sweep.process(first);
            let hp_gain = (1.0 - 2.0 * mode).min(0.0);
            let bp_gain = 1.0 - 2.0 * (mode - 0.5).abs();
            let lp_gain = (1.0 - 2.0 * mode).max(0.0);
            *o = hp_gain * swept.highpass + bp_gain * swept.bandpass + lp_gain * swept.lowpass;
            *a = self.bands[0].process(first).bandpass + self.bands[1].process(second).bandpass;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::drums::filters::testing::{params, peak, play};
    use super::*;

    fn loudness(rate: u32, timbre: f32) -> (f32, f32) {
        let mut noise = FilteredNoiseEngine::new(rate);
        // A wide band, so four seconds of noise measure its loudness well.
        let p = EngineParameters {
            timbre,
            morph: 0.0,
            sample_period: 1.0 / rate as f32,
            ..params(60.0, false, true)
        };
        let blocks = rate as usize / ENGINE_BLOCK;
        play(|p, o, a| noise.render(p, o, a), &p, blocks / 4);
        let (out, aux) = play(|p, o, a| noise.render(p, o, a), &p, 4 * blocks);
        let db = |x: &[f32]| 10.0 * (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).log10();
        (db(&out), db(&aux))
    }

    #[test]
    fn it_sounds_steadily_and_bounded() {
        let mut noise = FilteredNoiseEngine::new(48_000);
        let p = params(60.0, false, true);
        play(|p, o, a| noise.render(p, o, a), &p, 400);
        let (out, aux) = play(|p, o, a| noise.render(p, o, a), &p, 4_000);
        for x in [&out, &aux] {
            let halves = [&x[..24_000], &x[24_000..]].map(peak);
            assert!(halves[0] > 0.05 && halves[0] < 4.0, "{halves:?}");
            assert!((halves[0] / halves[1] - 1.0).abs() < 0.5, "{halves:?}");
        }
    }

    /// From grainy held noise to white, it is as loud at any rate.
    #[test]
    fn it_sounds_alike_at_any_rate() {
        for timbre in [0.45, 0.7, 0.95] {
            let want = loudness(48_000, timbre);
            for rate in [44_100, 96_000] {
                let got = loudness(rate, timbre);
                assert!(
                    (got.0 - want.0).abs() < 1.0 && (got.1 - want.1).abs() < 1.0,
                    "timbre {timbre} at {rate} Hz: {got:?} against {want:?}"
                );
            }
        }
    }

    /// `harmonics` from low-pass to high-pass: the sound gets brighter.
    #[test]
    fn harmonics_sweeps_low_to_high_pass() {
        let crossings = |harmonics: f32| {
            let mut noise = FilteredNoiseEngine::new(48_000);
            let p = EngineParameters {
                harmonics,
                timbre: 0.9,
                morph: 0.0,
                ..params(72.0, false, true)
            };
            let (out, _) = play(|p, o, a| noise.render(p, o, a), &p, 4_000);
            out.windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        assert!(crossings(0.95) > 2 * crossings(0.05));
    }
}
