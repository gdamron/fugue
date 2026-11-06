//! Ported from Mutable Instruments' `plaits/dsp/drums/synthetic_snare_drum.h`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A model of a classic hybrid snare: two detuned, softly clipped
//! triangles for the shell, swept by a pitch blip, beside band-limited
//! noise that holds a moment before it decays.

use super::filters::{flush, tan_fast, BlockGlide, ModeFilter, OnePoleFilter, Rate};
use crate::dsp::Rng;
use crate::modules::macro_oscillator::engine::semitones_to_ratio;

/// One block's settings, each 0 to 1 unless noted.
pub(super) struct SyntheticSnareSettings {
    pub sustain: bool,
    pub trigger: bool,
    pub accent: f32,
    /// The pitch, in cycles per sample.
    pub f0: f32,
    pub fm_amount: f32,
    pub decay: f32,
    pub snappy: f32,
}

pub(super) struct SyntheticSnareDrum {
    rate: Rate,
    rng: Rng,
    noise_gain: f32,
    phases: [f32; 2],
    drum_amplitude: f32,
    snare_amplitude: f32,
    fm: f32,
    hold: usize,
    sustain_gain: BlockGlide,
    drum_level: BlockGlide,
    snare_level: BlockGlide,
    drum_lp: OnePoleFilter,
    snare_hp: OnePoleFilter,
    snare_lp: ModeFilter,
}

/// A triangle, offset and softly clipped toward a sine.
#[inline]
fn distorted_sine(phase: f32) -> f32 {
    let triangle = (if phase < 0.5 { phase } else { 1.0 - phase }) * 4.0 - 1.3;
    2.0 * triangle / (1.0 + triangle.abs())
}

impl SyntheticSnareDrum {
    pub fn new(sample_rate: u32) -> Self {
        let rate = Rate::new(sample_rate);
        Self {
            rate,
            rng: Rng::new(0x5e1f),
            noise_gain: rate.noise_gain(),
            phases: [0.0; 2],
            drum_amplitude: 0.0,
            snare_amplitude: 0.0,
            fm: 0.0,
            hold: 0,
            sustain_gain: BlockGlide::default(),
            drum_level: BlockGlide::default(),
            snare_level: BlockGlide::default(),
            drum_lp: OnePoleFilter::default(),
            snare_hp: OnePoleFilter::default(),
            snare_lp: ModeFilter::default(),
        }
    }

    pub fn render(&mut self, s: &SyntheticSnareSettings, out: &mut [f32]) {
        let size = out.len();
        let decay = s.decay;
        let decay_xt = decay * (1.0 + decay * (decay - 1.0));
        let fm_amount = s.fm_amount * s.fm_amount;
        let drum_decay = 1.0
            - self.rate.per_second(0.015)
                * semitones_to_ratio(-decay_xt * 72.0 - fm_amount * 12.0 + s.snappy * 7.0);
        let snare_decay =
            1.0 - self.rate.per_second(0.01) * semitones_to_ratio(-decay * 60.0 - s.snappy * 7.0);
        let fm_decay = 1.0 - self.rate.per_second(0.007);

        let snappy = (s.snappy * 1.1 - 0.05).clamp(0.0, 1.0);
        self.drum_level.start((1.0 - snappy).sqrt(), size);
        self.snare_level.start(snappy.sqrt(), size);

        let snare_f_min = (10.0 * s.f0).min(0.5);
        let snare_f_max = (35.0 * s.f0).min(0.5);
        self.snare_hp.set_g(tan_fast(snare_f_min));
        self.snare_lp
            .set_g_q(tan_fast(snare_f_max), 0.5 + 2.0 * snappy);
        self.drum_lp.set_g(tan_fast(3.0 * s.f0));

        if s.trigger {
            self.drum_amplitude = 0.3 + 0.7 * s.accent;
            self.snare_amplitude = self.drum_amplitude;
            self.fm = 1.0;
            self.phases = [0.0; 2];
            self.hold = self.rate.samples(0.04 + decay * 0.03);
        }
        self.sustain_gain.start(s.accent * decay, size);

        let f0_48k = self.rate.at_48k(s.f0);
        let reset_noise_amount = ((0.125 - f0_48k) * 8.0).clamp(0.0, 1.0).powi(2) * fm_amount;

        for (i, o) in out.iter_mut().enumerate() {
            let gain = self.sustain_gain.next();
            if s.sustain {
                self.snare_amplitude = gain;
                self.drum_amplitude = gain;
                self.fm = 0.0;
            } else {
                // The drum's long tail decays on every other sample.
                if self.drum_amplitude > 0.03 || (size - 1 - i).is_multiple_of(2) {
                    self.drum_amplitude = flush(self.drum_amplitude * drum_decay);
                }
                if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.snare_amplitude = flush(self.snare_amplitude * snare_decay);
                }
                self.fm = flush(self.fm * fm_decay);
            }

            // The oscillators reset each other a little, as the circuit's
            // shared reset line does.
            let mut reset_noise = 0.0;
            reset_noise += if self.phases[0] > 0.5 { -1.0 } else { 1.0 };
            reset_noise += if self.phases[1] > 0.5 { -1.0 } else { 1.0 };
            reset_noise *= reset_noise_amount * 0.025;

            let f = s.f0 * (1.0 + fm_amount * (4.0 * self.fm));
            self.phases[0] += f;
            self.phases[1] += f * 1.47;
            for phase in &mut self.phases {
                if reset_noise_amount > 0.1 {
                    if *phase >= 1.0 + reset_noise {
                        *phase = 1.0 - *phase;
                    }
                } else if *phase >= 1.0 {
                    *phase -= 1.0;
                }
            }

            let mut drum = -0.1;
            drum += distorted_sine(self.phases[0]) * 0.60;
            drum += distorted_sine(self.phases[1]) * 0.25;
            drum *= self.drum_amplitude * self.drum_level.next();
            let drum = self.drum_lp.lowpass(drum);

            let noise = self.rng.unit() * self.noise_gain;
            let snare = self.snare_lp.process(noise).lowpass;
            let snare = self.snare_hp.highpass(snare);
            let snare = (snare + 0.1) * (self.snare_amplitude + self.fm) * self.snare_level.next();
            *o = snare + drum;
        }
    }
}
