//! Ported from Mutable Instruments' `plaits/dsp/drums/analog_snare_drum.h`
//! and `plaits/dsp/oscillator/sine_oscillator.h` (MIT, Copyright 2016
//! Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A model of a classic analog snare circuit: a trigger pulse rings a few
//! tuned shell modes, beside a burst of filtered noise for the snares.
//! Un-triggered, sines stand in for the modes.

use super::filters::{flush, soft_clip, tan_fast, BlockGlide, ModeFilter, Rate};
use crate::dsp::{sine, Rng};
use crate::modules::macro_oscillator::engine::semitones_to_ratio;

/// The shell's modes, as ratios of its pitch.
const MODE_RATIOS: [f32; MODES] = [1.00, 2.00, 3.18, 4.16, 5.62];
const MODES: usize = 5;

/// One block's settings, each 0 to 1 unless noted.
pub(super) struct AnalogSnareSettings {
    pub sustain: bool,
    pub trigger: bool,
    pub accent: f32,
    /// The pitch, in cycles per sample.
    pub f0: f32,
    pub tone: f32,
    pub decay: f32,
    /// How much of the sound is the snares rather than the shell.
    pub snappy: f32,
}

pub(super) struct AnalogSnareDrum {
    rate: Rate,
    rng: Rng,
    noise_gain: f32,
    trigger_pulse: usize,
    pulse_decay: f32,
    pulse_filter: f32,

    pulse_remaining: usize,
    pulse: f32,
    pulse_height: f32,
    pulse_lp: f32,
    noise_envelope: f32,
    sustain_gain: BlockGlide,
    gains: [BlockGlide; MODES],
    leak: BlockGlide,
    snappy: BlockGlide,
    modes: [ModeFilter; MODES],
    phases: [f32; MODES],
    noise_filter: ModeFilter,
}

/// Each mode's level for `tone`: two modes up to two thirds, then the
/// upper three fade in.
fn mode_gains(tone: f32) -> [f32; MODES] {
    let mut gain = [0.0; MODES];
    if tone < 2.0 / 3.0 {
        let tone = tone * 1.5;
        gain[0] = 1.5 + (1.0 - tone) * (1.0 - tone) * 4.5;
        gain[1] = 2.0 * tone + 0.15;
    } else {
        let mut tone = (tone - 2.0 / 3.0) * 3.0;
        gain[0] = 1.5 - tone * 0.5;
        gain[1] = 2.15 - tone * 0.7;
        for g in &mut gain[2..] {
            *g = tone;
            tone *= tone;
        }
    }
    gain
}

impl AnalogSnareDrum {
    pub fn new(sample_rate: u32) -> Self {
        let rate = Rate::new(sample_rate);
        Self {
            rate,
            rng: Rng::new(0x5a4e),
            noise_gain: rate.noise_gain(),
            trigger_pulse: rate.samples(1.0e-3),
            pulse_decay: 1.0 - rate.per_second(0.1e-3),
            pulse_filter: rate.lag(0.75),
            pulse_remaining: 0,
            pulse: 0.0,
            pulse_height: 0.0,
            pulse_lp: 0.0,
            noise_envelope: 0.0,
            sustain_gain: BlockGlide::default(),
            gains: Default::default(),
            leak: BlockGlide::default(),
            snappy: BlockGlide::default(),
            modes: Default::default(),
            phases: [0.0; MODES],
            noise_filter: ModeFilter::default(),
        }
    }

    pub fn render(&mut self, s: &AnalogSnareSettings, out: &mut [f32]) {
        let size = out.len();
        let to_48k = self.rate.at_48k(1.0);
        let decay = s.decay;
        let decay_xt = decay * (1.0 + decay * (decay - 1.0));
        let q = 2_000.0 * semitones_to_ratio(decay_xt * 84.0);
        let noise_decay_48k = 1.0 - 0.0017 * semitones_to_ratio(-decay * (50.0 + s.snappy * 10.0));
        let noise_decay = noise_decay_48k.powf(to_48k.recip());
        self.leak.start(s.snappy * (2.0 - s.snappy) * 0.1, size);
        self.snappy
            .start((s.snappy * 1.1 - 0.05).clamp(0.0, 1.0), size);

        if s.trigger {
            self.pulse_remaining = self.trigger_pulse;
            self.pulse_height = 3.0 + 7.0 * s.accent;
            self.noise_envelope = 2.0;
        }

        let mut f = [0.0; MODES];
        for (i, mode) in self.modes.iter_mut().enumerate() {
            f[i] = (s.f0 * MODE_RATIOS[i]).min(0.499);
            let q = if i == 0 { q } else { q * 0.25 };
            mode.set_g_q(tan_fast(f[i]), 1.0 + f[i] * to_48k * q);
        }
        for (glide, gain) in self.gains.iter_mut().zip(mode_gains(s.tone)) {
            glide.start(gain, size);
        }
        let f_noise = (s.f0 * 16.0).clamp(0.0, 0.499);
        self.noise_filter
            .set_g_q(tan_fast(f_noise), 1.0 + f_noise * to_48k * 1.5);
        self.sustain_gain.start(s.accent * decay, size);

        for o in out.iter_mut() {
            let pulse = if self.pulse_remaining > 0 {
                self.pulse_remaining -= 1;
                self.pulse = if self.pulse_remaining > 0 {
                    self.pulse_height
                } else {
                    self.pulse_height - 1.0
                };
                self.pulse
            } else {
                self.pulse = flush(self.pulse * self.pulse_decay);
                self.pulse
            };
            let gain = self.sustain_gain.next();
            let leak = self.leak.next();
            let snappy = self.snappy.next();
            self.pulse_lp += self.pulse_filter * (pulse - self.pulse_lp);

            let mut shell = 0.0;
            let voices = self
                .modes
                .iter_mut()
                .zip(&mut self.gains)
                .zip(&mut self.phases);
            for (i, ((mode, level), phase)) in voices.enumerate() {
                let level = level.next();
                let excitation = if i == 0 {
                    (pulse - self.pulse_lp) + 0.006 * pulse
                } else {
                    0.026 * pulse
                };
                shell += level
                    * if s.sustain {
                        *phase += f[i];
                        *phase -= phase.floor();
                        sine(*phase) * gain * 0.25
                    } else {
                        mode.process(excitation).bandpass + excitation * leak
                    };
            }
            let shell = soft_clip(shell);

            let noise = (2.0 * self.rng.unit() - 1.0).max(0.0) * self.noise_gain;
            self.noise_envelope = flush(self.noise_envelope * noise_decay);
            let envelope = if s.sustain { gain } else { self.noise_envelope };
            let noise = self
                .noise_filter
                .process(noise * envelope * snappy * 2.0)
                .bandpass;
            *o = noise + shell * (1.0 - snappy);
        }
    }
}
