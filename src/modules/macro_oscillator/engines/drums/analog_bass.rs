//! Ported from Mutable Instruments' `plaits/dsp/drums/analog_bass_drum.h`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A model of a classic analog bass drum circuit: a trigger pulse rings a
//! bridged-T resonator whose pitch the pulse itself bends at the attack.
//! Un-triggered, a sine stands in for the resonator.

use super::filters::{one_pole, tan_dirty, BlockGlide, ModeFilter, Rate};
use crate::dsp::sine;
use crate::modules::macro_oscillator::engine::semitones_to_ratio;

/// One block's settings, each 0 to 1 unless noted.
pub(super) struct AnalogBassSettings {
    pub sustain: bool,
    pub trigger: bool,
    pub accent: f32,
    /// The pitch, in cycles per sample.
    pub f0: f32,
    pub tone: f32,
    pub decay: f32,
    pub attack_fm: f32,
    pub self_fm: f32,
}

pub(super) struct AnalogBassDrum {
    rate: Rate,
    trigger_pulse: usize,
    fm_pulse: usize,
    pulse_decay: f32,
    pulse_filter: f32,
    retrig_decay: f32,

    pulse_remaining: usize,
    fm_pulse_remaining: usize,
    pulse: f32,
    pulse_height: f32,
    pulse_lp: f32,
    fm_pulse_lp: f32,
    retrig_pulse: f32,
    lp_out: f32,
    tone_lp: f32,
    tone_f: BlockGlide,
    sustain_gain: BlockGlide,
    leak: BlockGlide,
    resonator: ModeFilter,
    phase: f32,
}

/// Bends a negative swing the way a diode and its resistor do.
#[inline]
fn diode(x: f32) -> f32 {
    if x >= 0.0 {
        x
    } else {
        let x = 2.0 * x;
        0.7 * x / (1.0 + x.abs())
    }
}

impl AnalogBassDrum {
    pub fn new(sample_rate: u32) -> Self {
        let rate = Rate::new(sample_rate);
        Self {
            rate,
            trigger_pulse: rate.samples(1.0e-3),
            fm_pulse: rate.samples(6.0e-3),
            pulse_decay: 1.0 - rate.per_second(0.2e-3),
            pulse_filter: rate.per_second(0.1e-3),
            retrig_decay: 1.0 - rate.per_second(0.05),
            pulse_remaining: 0,
            fm_pulse_remaining: 0,
            pulse: 0.0,
            pulse_height: 0.0,
            pulse_lp: 0.0,
            fm_pulse_lp: 0.0,
            retrig_pulse: 0.0,
            lp_out: 0.0,
            tone_lp: 0.0,
            tone_f: BlockGlide::default(),
            sustain_gain: BlockGlide::default(),
            leak: BlockGlide::default(),
            resonator: ModeFilter::default(),
            phase: 0.0,
        }
    }

    pub fn render(&mut self, s: &AnalogBassSettings, out: &mut [f32]) {
        let size = out.len();
        let f0_48k = self.rate.at_48k(s.f0);
        let scale = 0.001 / f0_48k.max(1e-6);
        let q = 1_500.0 * semitones_to_ratio(s.decay * 80.0);
        let tone_f = (4.0 * f0_48k * semitones_to_ratio(s.tone * 108.0)).min(1.0);
        self.tone_f.start(self.rate.coefficient(tone_f), size);
        self.leak.start(0.08 * (s.tone + 0.25), size);

        if s.trigger {
            self.pulse_remaining = self.trigger_pulse;
            self.fm_pulse_remaining = self.fm_pulse;
            self.pulse_height = 3.0 + 7.0 * s.accent;
            self.lp_out = 0.0;
        }
        self.sustain_gain.start(s.accent * s.decay, size);
        let to_48k = self.rate.at_48k(1.0);

        for o in out.iter_mut() {
            let mut pulse = if self.pulse_remaining > 0 {
                self.pulse_remaining -= 1;
                self.pulse = if self.pulse_remaining > 0 {
                    self.pulse_height
                } else {
                    self.pulse_height - 1.0
                };
                self.pulse
            } else {
                self.pulse *= self.pulse_decay;
                self.pulse
            };
            if s.sustain {
                pulse = 0.0;
            }
            one_pole(&mut self.pulse_lp, pulse, self.pulse_filter);
            let pulse = diode((pulse - self.pulse_lp) + pulse * 0.044);

            let mut fm_pulse = 0.0;
            if self.fm_pulse_remaining > 0 {
                self.fm_pulse_remaining -= 1;
                fm_pulse = 1.0;
                self.retrig_pulse = if self.fm_pulse_remaining > 0 {
                    0.0
                } else {
                    -0.8
                };
            } else {
                self.retrig_pulse *= self.retrig_decay;
            }
            if s.sustain {
                fm_pulse = 0.0;
            }
            one_pole(&mut self.fm_pulse_lp, fm_pulse, self.pulse_filter);

            let punch = 0.7 + diode(10.0 * self.lp_out - 1.0);
            let attack_fm = self.fm_pulse_lp * 1.7 * s.attack_fm;
            let self_fm = punch * 0.08 * s.self_fm;
            let f = (s.f0 * (1.0 + attack_fm + self_fm)).clamp(0.0, 0.4);

            let gain = self.sustain_gain.next();
            let resonator_out = if s.sustain {
                self.phase += f;
                self.phase -= self.phase.floor();
                self.lp_out = gain * sine(self.phase + 0.25);
                gain * sine(self.phase)
            } else {
                self.resonator.set_g_q(tan_dirty(f), 1.0 + q * f * to_48k);
                let modes = self
                    .resonator
                    .process((pulse - self.retrig_pulse * 0.2) * scale);
                self.lp_out = modes.lowpass;
                modes.bandpass
            };
            let tone_f = self.tone_f.next();
            *o = one_pole(
                &mut self.tone_lp,
                pulse * self.leak.next() + resonator_out,
                tone_f,
            );
        }
    }
}
