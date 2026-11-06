//! Ported from Mutable Instruments' `plaits/dsp/drums/synthetic_bass_drum.h`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A model of a classic digital-analog hybrid bass drum: a sine, bent
//! towards a soft triangle by `dirtiness`, swept down from a pitch blip
//! through a transistor VCA, with a click and a breath of noise on top.

use super::super::filters::{flush, one_pole, slope, tan_fast, BlockGlide, ModeFilter, Rate};
use crate::dsp::{sine, Rng};
use crate::modules::macro_oscillator::engine::semitones_to_ratio;

/// One block's settings, each 0 to 1 unless noted.
pub(super) struct SyntheticBassSettings {
    pub sustain: bool,
    pub trigger: bool,
    pub accent: f32,
    /// The pitch, in cycles per sample.
    pub f0: f32,
    pub tone: f32,
    pub decay: f32,
    pub dirtiness: f32,
    pub fm_amount: f32,
    pub fm_decay: f32,
}

/// The click of the beater: a smoothed step, high-passed and rounded off.
struct Click {
    lp: f32,
    hp: f32,
    rise: f32,
    fall: f32,
    high_pass: f32,
    filter: ModeFilter,
}

impl Click {
    fn new(rate: Rate, sample_rate: u32) -> Self {
        let mut filter = ModeFilter::default();
        filter.set_g_q(tan_fast(5_000.0 / sample_rate.max(1) as f32), 2.0);
        Self {
            lp: 0.0,
            hp: 0.0,
            rise: rate.coefficient(0.5),
            fall: rate.coefficient(0.1),
            high_pass: rate.coefficient(0.04),
            filter,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        slope(&mut self.lp, x, self.rise, self.fall);
        one_pole(&mut self.hp, self.lp, self.high_pass);
        self.filter.process(self.lp - self.hp).lowpass
    }
}

pub(super) struct SyntheticBassDrum {
    rate: Rate,
    rng: Rng,
    noise_gain: f32,
    transient_decay: f32,
    envelope_lp_f: f32,
    phase_noise_f: f32,
    noise_lp_f: f32,
    noise_hp_f: f32,
    body_pulse: usize,
    fm_pulse: usize,

    f0: BlockGlide,
    dirtiness: BlockGlide,
    transient_level: BlockGlide,
    sustain_gain: BlockGlide,
    phase: f32,
    phase_noise: f32,
    fm: f32,
    fm_lp: f32,
    body_env: f32,
    body_env_lp: f32,
    transient_env: f32,
    transient_env_lp: f32,
    tone_lp: f32,
    tone_f: BlockGlide,
    noise_lp: f32,
    noise_hp: f32,
    click: Click,
    body_pulse_remaining: usize,
    fm_pulse_remaining: usize,
}

/// A sine, bent by `dirtiness` toward a soft-clipped triangle and nudged
/// by `phase_noise`.
#[inline]
fn distorted_sine(phase: f32, phase_noise: f32, dirtiness: f32) -> f32 {
    let phase = phase + phase_noise * dirtiness;
    let phase = phase - phase.floor();
    let triangle = (if phase < 0.5 { phase } else { 1.0 - phase }) * 4.0 - 1.0;
    let soft = 2.0 * triangle / (1.0 + triangle.abs());
    let clean = sine(phase + 0.75);
    soft + (1.0 - dirtiness) * (clean - soft)
}

/// A transistor VCA's curve, with the gain that leaks through it.
#[inline]
fn transistor_vca(s: f32, gain: f32) -> f32 {
    let s = (s - 0.6) * gain;
    3.0 * s / (2.0 + s.abs()) + gain * 0.3
}

impl SyntheticBassDrum {
    pub fn new(sample_rate: u32) -> Self {
        let rate = Rate::new(sample_rate);
        Self {
            rate,
            rng: Rng::new(0xba55),
            noise_gain: rate.noise_gain(),
            transient_decay: 1.0 - rate.per_second(0.005),
            envelope_lp_f: rate.coefficient(0.1),
            phase_noise_f: rate.coefficient(0.002),
            noise_lp_f: rate.coefficient(0.05),
            noise_hp_f: rate.coefficient(0.005),
            body_pulse: rate.samples(0.001),
            fm_pulse: rate.samples(0.0013),
            f0: BlockGlide::default(),
            dirtiness: BlockGlide::default(),
            transient_level: BlockGlide::default(),
            sustain_gain: BlockGlide::default(),
            phase: 0.0,
            phase_noise: 0.0,
            fm: 0.0,
            fm_lp: 0.0,
            body_env: 0.0,
            body_env_lp: 0.0,
            transient_env: 0.0,
            transient_env_lp: 0.0,
            tone_lp: 0.0,
            tone_f: BlockGlide::default(),
            noise_lp: 0.0,
            noise_hp: 0.0,
            click: Click::new(rate, sample_rate),
            body_pulse_remaining: 0,
            fm_pulse_remaining: 0,
        }
    }

    pub fn render(&mut self, s: &SyntheticBassSettings, out: &mut [f32]) {
        let size = out.len();
        let decay = s.decay * s.decay;
        let fm_envelope_decay = s.fm_decay * s.fm_decay;
        let f0_48k = self.rate.at_48k(s.f0);
        self.f0.start(s.f0, size);
        self.dirtiness
            .start(s.dirtiness * (1.0 - 8.0 * f0_48k).max(0.0), size);
        self.transient_level.start(s.tone, size);

        let fm_decay = 1.0
            - self
                .rate
                .per_second(0.008 * (1.0 + fm_envelope_decay * 4.0));
        let body_env_decay = 1.0 - self.rate.per_second(0.02) * semitones_to_ratio(-decay * 60.0);
        let tone_f = (4.0 * f0_48k * semitones_to_ratio(s.tone * 108.0)).min(1.0);
        self.tone_f.start(self.rate.coefficient(tone_f), size);

        if s.trigger {
            self.fm = 1.0;
            self.body_env = 0.3 + 0.7 * s.accent;
            self.transient_env = self.body_env;
            self.body_pulse_remaining = self.body_pulse;
            self.fm_pulse_remaining = self.fm_pulse;
        }
        self.sustain_gain.start(s.accent * decay, size);

        for o in out.iter_mut() {
            let noise = (self.rng.unit() - 0.5) * self.noise_gain;
            one_pole(&mut self.phase_noise, noise, self.phase_noise_f);
            let f0 = self.f0.next();
            let dirtiness = self.dirtiness.next();
            let transient_level = self.transient_level.next();
            let gain = self.sustain_gain.next();

            let mut mix = 0.0;
            if s.sustain {
                self.phase += f0;
                self.phase -= self.phase.floor();
                let body = distorted_sine(self.phase, self.phase_noise, dirtiness);
                mix -= transistor_vca(body, gain);
            } else {
                if self.fm_pulse_remaining > 0 {
                    self.fm_pulse_remaining -= 1;
                    self.phase = 0.25;
                } else {
                    self.fm = flush(self.fm * fm_decay);
                    let fm = 1.0 + s.fm_amount * 3.5 * self.fm_lp;
                    self.phase += (f0 * fm).min(0.5);
                    self.phase -= self.phase.floor();
                }
                if self.body_pulse_remaining > 0 {
                    self.body_pulse_remaining -= 1;
                } else {
                    self.body_env = flush(self.body_env * body_env_decay);
                    self.transient_env = flush(self.transient_env * self.transient_decay);
                }
                one_pole(&mut self.body_env_lp, self.body_env, self.envelope_lp_f);
                one_pole(
                    &mut self.transient_env_lp,
                    self.transient_env,
                    self.envelope_lp_f,
                );
                one_pole(&mut self.fm_lp, self.fm, self.envelope_lp_f);

                let body = distorted_sine(self.phase, self.phase_noise, dirtiness);
                let step = if self.body_pulse_remaining > 0 {
                    0.0
                } else {
                    1.0
                };
                let breath = self.rng.unit() * self.noise_gain;
                one_pole(&mut self.noise_lp, breath, self.noise_lp_f);
                one_pole(&mut self.noise_hp, self.noise_lp, self.noise_hp_f);
                let transient = self.click.process(step) + self.noise_lp - self.noise_hp;

                mix -= transistor_vca(body, self.body_env_lp);
                mix -= transient * self.transient_env_lp * transient_level;
            }
            *o = one_pole(&mut self.tone_lp, mix, self.tone_f.next());
        }
    }
}
