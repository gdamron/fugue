//! A small, fast, seeded random number generator for the audio thread.
//!
//! A permuted congruential generator (O'Neill, "PCG: A Family of Simple Fast
//! Space-Efficient Statistically Good Algorithms for Random Number
//! Generation", 2014): a 64-bit linear congruential state whose high bits are
//! shuffled by a data-dependent rotation on the way out (the XSH-RR output).
//! It is a few integer instructions per draw, never allocates, and two
//! generators from the same seed give the same stream.
//!
//! Streams are stable within a build, not promised across versions: a
//! seed reproduces a render, not a render from an older release.

/// The LCG multiplier (Knuth's MMIX constant).
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
/// The LCG increment; any odd number gives the full period.
const INCREMENT: u64 = 1_442_695_040_888_963_407;
/// 2^-24: a 24-bit integer scaled to `[0, 1)` is exact in an `f32`.
const UNIT: f32 = 1.0 / (1u32 << 24) as f32;

/// A seeded random number generator cheap enough to draw from every sample.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator whose stream is fixed by `seed`. Nearby seeds (0, 1, 2)
    /// give unrelated streams: the seed is scrambled before use.
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: scramble(seed).wrapping_add(INCREMENT),
        };
        rng.next_u32();
        rng
    }

    /// The next 32 random bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        let shifted = (((old >> 18) ^ old) >> 27) as u32;
        shifted.rotate_right((old >> 59) as u32)
    }

    /// A value uniform in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * UNIT
    }

    /// A value uniform in `[-1, 1)`: white noise at full scale, whose RMS
    /// is `1/√3` (about 0.577).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

/// SplitMix64's finalizer (Steele, Lea and Flood, "Fast Splittable
/// Pseudorandom Number Generators", 2014, with the mixing constants Vigna
/// published for it): spreads a seed's bits over the whole state, so small
/// seeds don't start in similar states.
fn scramble(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn a_seed_reproduces_its_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn neighbouring_seeds_give_unrelated_streams() {
        let mut a = Rng::new(0);
        let mut b = Rng::new(1);
        let same = (0..1000).filter(|_| a.next_u32() == b.next_u32()).count();
        assert_eq!(same, 0);
    }

    #[test]
    fn bipolar_values_fill_the_range_with_the_expected_spread() {
        let mut rng = Rng::new(7);
        let n = 480_000;
        let (mut sum, mut squares) = (0.0f64, 0.0f64);
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for _ in 0..n {
            let value = rng.bipolar();
            sum += f64::from(value);
            squares += f64::from(value) * f64::from(value);
            low = low.min(value);
            high = high.max(value);
        }
        let mean = sum / n as f64;
        let rms = (squares / n as f64).sqrt();
        assert!(mean.abs() < 0.005, "mean {mean}");
        assert!((rms - 1.0 / 3f64.sqrt()).abs() < 0.005, "rms {rms}");
        assert!((-1.0..-0.999).contains(&low), "low {low}");
        assert!(high < 1.0 && high > 0.999, "high {high}");
    }
}
