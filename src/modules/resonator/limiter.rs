//! Ported from Mutable Instruments' `rings/dsp/limiter.h` (MIT, Copyright
//! 2015 Emilie Gillet), with `SoftLimit` from `stmlib/dsp/dsp.h` (MIT,
//! Copyright 2012 Emilie Gillet), adapted to Fugue's sample rates. See
//! THIRD_PARTY_NOTICES.md.

/// The output limiter: a peak follower that turns the gain down when the
/// sound passes full scale, then a soft clip.
pub(super) struct Limiter {
    peak: f32,
    attack: f32,
    release: f32,
}

impl Limiter {
    pub(super) fn new(sample_rate: u32) -> Self {
        let scale = 48_000.0 / sample_rate.max(1) as f32;
        Self {
            peak: 0.5,
            attack: (0.05 * scale).min(1.0),
            release: (0.00002 * scale).min(1.0),
        }
    }

    /// Limits one stereo frame (the mono sum is kept in range too).
    #[inline]
    pub(super) fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left
            .abs()
            .max(right.abs())
            .max((left - right).abs())
            .max((left + right).abs());
        let error = peak - self.peak;
        self.peak += if error > 0.0 {
            self.attack
        } else {
            self.release
        } * error;
        // Only whether the peak passes full scale matters, so a long silence
        // is not left releasing through subnormals.
        if self.peak < 1.0e-20 {
            self.peak = 0.0;
        }
        let gain = if self.peak <= 1.0 {
            0.8
        } else {
            0.8 / self.peak
        };
        (soft_limit(left * gain), soft_limit(right * gain))
    }
}

/// A gentle saturating curve, near-linear for small values.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_releases_to_zero_not_subnormals() {
        let mut limiter = Limiter::new(48_000);
        for _ in 0..48_000 * 120 {
            limiter.process(0.0, 0.0);
        }
        assert_eq!(limiter.peak, 0.0);
    }
}
