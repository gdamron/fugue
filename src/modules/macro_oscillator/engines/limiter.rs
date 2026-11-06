//! Ported from Mutable Instruments' `stmlib/dsp/limiter.h` (MIT, Copyright
//! 2015 Emilie Gillet), with its coefficients moved to Fugue's sample rates.
//! See THIRD_PARTY_NOTICES.md.

/// The limiter's attack and release per sample at 48 kHz, and its output
/// trim: peaks are held to the trim.
const ATTACK: f32 = 0.05;
const RELEASE: f32 = 0.000_02;
const TRIM: f32 = 0.8;

/// A peak follower that turns a loud engine down to full scale, for the
/// engines whose level swings widely with their settings.
pub(super) struct Limiter {
    peak: f32,
    attack: f32,
    release: f32,
}

impl Limiter {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        // The per-sample coefficients, moved to this rate.
        let per_sample = |c: f32| 1.0 - (1.0 - c).powf(48_000.0 / rate);
        Self {
            peak: 0.5,
            attack: per_sample(ATTACK),
            release: per_sample(RELEASE),
        }
    }

    /// Limits `block` in place after a gain of `drive`.
    pub fn process(&mut self, drive: f32, block: &mut [f32]) {
        for o in block.iter_mut() {
            let s = *o * drive;
            let error = s.abs() - self.peak;
            self.peak += error
                * if error > 0.0 {
                    self.attack
                } else {
                    self.release
                };
            // A long silence settles at 0 rather than releasing through
            // subnormals.
            if self.peak < 1.0e-20 {
                self.peak = 0.0;
            }
            let gain = if self.peak <= 1.0 {
                1.0
            } else {
                1.0 / self.peak
            };
            *o = s * gain * TRIM;
        }
    }
}
