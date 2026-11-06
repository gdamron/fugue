//! Ported from Mutable Instruments' `plaits/dsp/drums/hi_hat.h` (the
//! `HiHat` template, `SwingVCA` and `LinearVCA`; MIT, Copyright 2016
//! Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A hi-hat: clangorous metallic noise through a band-pass, roughened by
//! a little clocked noise, then a decaying VCA and a high-pass.

use super::filters::{flush, tan_accurate, BlockGlide, ModeFilter, Rate};
use super::metallic_noise::{RingModNoise, SquareNoise};
use crate::dsp::Rng;
use crate::modules::macro_oscillator::engine::{semitones_to_ratio, ENGINE_BLOCK};

/// One block's settings, each 0 to 1 unless noted.
pub(super) struct HiHatSettings {
    pub sustain: bool,
    pub trigger: bool,
    pub accent: f32,
    /// The pitch, in cycles per sample.
    pub f0: f32,
    pub tone: f32,
    pub decay: f32,
    /// How much clocked noise roughens the metallic tone.
    pub noisiness: f32,
}

/// The metallic source under a [`HiHat`].
enum Metal {
    Squares(SquareNoise),
    Rings(RingModNoise),
}

/// The two hi-hats the engine pairs.
pub(super) enum HiHatKind {
    /// Square-wave noise through a resonant band-pass and a VCA that
    /// swings, as the classic analog circuit does.
    Analog,
    /// Ring-modulated noise through a linear VCA whose decay shortens
    /// once it falls past half, like a choked cymbal.
    Ringing,
}

pub(super) struct HiHat {
    rate: Rate,
    sample_rate: f32,
    rng: Rng,
    metal: Metal,
    swing: bool,
    resonance: bool,
    two_stage: bool,

    envelope: f32,
    noise_clock: f32,
    noise_sample: f32,
    sustain_gain: BlockGlide,
    coloration: ModeFilter,
    high_pass: ModeFilter,
    square: [f32; ENGINE_BLOCK],
    saw: [f32; ENGINE_BLOCK],
}

impl HiHat {
    pub fn new(sample_rate: u32, kind: HiHatKind) -> Self {
        let analog = matches!(kind, HiHatKind::Analog);
        Self {
            rate: Rate::new(sample_rate),
            sample_rate: sample_rate.max(1) as f32,
            rng: Rng::new(if analog { 0x4848 } else { 0x5252 }),
            metal: if analog {
                Metal::Squares(SquareNoise::default())
            } else {
                Metal::Rings(RingModNoise::new(sample_rate))
            },
            swing: analog,
            resonance: analog,
            two_stage: !analog,
            envelope: 0.0,
            noise_clock: 0.0,
            noise_sample: 0.0,
            sustain_gain: BlockGlide::default(),
            coloration: ModeFilter::default(),
            high_pass: ModeFilter::default(),
            square: [0.0; ENGINE_BLOCK],
            saw: [0.0; ENGINE_BLOCK],
        }
    }

    /// Renders one block (at most [`ENGINE_BLOCK`] frames) into `out`.
    pub fn render(&mut self, s: &HiHatSettings, out: &mut [f32]) {
        let decay = s.decay;
        let envelope_decay = 1.0
            - self
                .rate
                .coefficient(0.003 * semitones_to_ratio(-decay * 84.0));
        let cut_decay = 1.0
            - self
                .rate
                .coefficient(0.0025 * semitones_to_ratio(-decay * 36.0));
        if s.trigger {
            self.envelope = (1.5 + 0.5 * (1.0 - decay)) * (0.3 + 0.7 * s.accent);
        }

        match &mut self.metal {
            Metal::Squares(noise) => noise.render(2.0 * s.f0, out),
            Metal::Rings(noise) => {
                let n = out.len();
                noise.render(2.0 * s.f0, &mut self.square[..n], &mut self.saw[..n], out);
            }
        }

        let cutoff = (150.0 / self.sample_rate * semitones_to_ratio(s.tone * 72.0))
            .clamp(0.0, 16_000.0 / self.sample_rate);
        let g = tan_accurate(cutoff);
        let q = if self.resonance {
            3.0 + 3.0 * s.tone
        } else {
            1.0
        };
        self.coloration.set_g_q(g, q);
        self.high_pass.set_g_q(g, 0.5);

        // Not part of the classic circuit: a little clocked noise mixed
        // into the oscillators, for more variety.
        let noisiness = s.noisiness * s.noisiness;
        let noise_f = (s.f0 * (16.0 + 16.0 * (1.0 - noisiness))).clamp(0.0, 0.5);
        self.sustain_gain.start(s.accent * decay, out.len());
        for o in out.iter_mut() {
            let mut x = self.coloration.process(*o).bandpass;
            self.noise_clock += noise_f;
            if self.noise_clock >= 1.0 {
                self.noise_clock -= 1.0;
                self.noise_sample = self.rng.unit() - 0.5;
            }
            x += noisiness * (self.noise_sample - x);

            self.envelope = flush(
                self.envelope
                    * if self.envelope > 0.5 || !self.two_stage {
                        envelope_decay
                    } else {
                        cut_decay
                    },
            );
            let sustain_gain = self.sustain_gain.next();
            let gain = if s.sustain {
                sustain_gain
            } else {
                self.envelope
            };
            let x = if self.swing {
                let x = x * if x > 0.0 { 4.0 } else { 0.1 };
                (x / (1.0 + x.abs()) + 0.1) * gain
            } else {
                x * gain
            };
            *o = self.high_pass.process(x).highpass;
        }
    }
}
