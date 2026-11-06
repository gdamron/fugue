//! Ported from Mutable Instruments' `plaits/dsp/noise/clocked_noise.h`
//! (MIT, Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.

use super::super::filters::{BlockGlide, Rate};
use crate::dsp::Rng;
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample};

/// Random values held for a clock period, each step band-limited, fading
/// to plain white noise as the clock nears the sample rate.
pub(super) struct ClockedNoise {
    rate: Rate,
    noise_gain: f32,
    rng: Rng,
    phase: f32,
    sample: f32,
    next_sample: f32,
    frequency: BlockGlide,
}

impl ClockedNoise {
    pub fn new(sample_rate: u32, seed: u64) -> Self {
        let rate = Rate::new(sample_rate);
        let mut frequency = BlockGlide::default();
        frequency.start(0.001, 1);
        frequency.next();
        Self {
            rate,
            noise_gain: rate.noise_gain(),
            rng: Rng::new(seed),
            phase: 0.0,
            sample: 0.0,
            next_sample: 0.0,
            frequency,
        }
    }

    /// Renders `out` at clock `frequency`, in cycles per sample; `sync`
    /// starts a new value at once.
    pub fn render(&mut self, sync: bool, frequency: f32, out: &mut [f32]) {
        self.frequency.start(frequency.clamp(0.0, 1.0), out.len());
        if sync {
            self.phase = 1.0;
        }
        let mut next_sample = self.next_sample;
        for o in out.iter_mut() {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            let frequency = self.frequency.next();
            let raw = self.rng.bipolar();
            // Above a quarter of 48 kHz the held values give way to white
            // noise, whose level per hertz falls as the rate rises.
            let raw_amount = (4.0 * (self.rate.at_48k(frequency) - 0.25)).clamp(0.0, 1.0);
            self.phase += frequency;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                let t = self.phase / frequency;
                let discontinuity = raw - self.sample;
                this_sample += discontinuity * this_blep_sample(t);
                next_sample += discontinuity * next_blep_sample(t);
                self.sample = raw;
            }
            next_sample += self.sample;
            *o = this_sample + raw_amount * (raw * self.noise_gain - this_sample);
        }
        self.next_sample = next_sample;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_clock_holds_its_values() {
        let mut noise = ClockedNoise::new(48_000, 1);
        let mut out = [0.0; 4_800];
        noise.render(false, 100.0 / 48_000.0, &mut out[..12]);
        noise.render(false, 100.0 / 48_000.0, &mut out);
        // About 100 new values a second: most samples repeat the last.
        let held = out.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(held > 4_600, "{held}");
        assert!(out.iter().all(|s| s.abs() <= 1.5));
    }

    #[test]
    fn sync_starts_a_new_value_at_once() {
        let mut noise = ClockedNoise::new(48_000, 2);
        let mut out = [0.0; 12];
        for _ in 0..100 {
            noise.render(false, 1.0 / 48_000.0, &mut out);
        }
        let before = out[11];
        noise.render(true, 1.0 / 48_000.0, &mut out);
        assert_ne!(out[2], before);
    }
}
