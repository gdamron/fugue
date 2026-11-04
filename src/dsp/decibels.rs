//! Fast conversions between decibels and linear level, for per-sample use.
//!
//! `powf`, `exp` and `log` are too slow to call for every sample of every
//! voice; these split a value into a power of two, which costs a few bit
//! operations, and a short polynomial for the rest. Relative error is below
//! 1e-6 for levels, and the dB of a level is within 1e-4 dB.

/// dB per doubling of level: 20·log10(2).
const DECIBELS_PER_OCTAVE: f32 = 6.020_6;
/// Octaves per dB: 1 / [`DECIBELS_PER_OCTAVE`].
const OCTAVES_PER_DECIBEL: f32 = 0.166_096_4;
/// The quietest level, and its dB: anything at or below reads as this.
pub const FLOOR_LEVEL: f32 = 1e-6;
pub const FLOOR_DECIBELS: f32 = -120.0;

/// The linear level of `decibels`: 0 dB is exactly 1.
#[inline]
pub fn decibels_to_level(decibels: f32) -> f32 {
    exp2(decibels * OCTAVES_PER_DECIBEL)
}

/// The dB of a linear `level`'s magnitude, never below
/// [`FLOOR_DECIBELS`] (silence and non-finite values read as the floor).
#[inline]
pub fn level_to_decibels(level: f32) -> f32 {
    let level = level.abs();
    if level > FLOOR_LEVEL && level.is_finite() {
        (log2(level) * DECIBELS_PER_OCTAVE).max(FLOOR_DECIBELS)
    } else {
        FLOOR_DECIBELS
    }
}

/// 2^`x`, for `x` within ±126 (clamped there): exactly 1 at 0, and exact at
/// every whole number.
#[inline]
pub fn exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let whole = x.round();
    let f = (x - whole) * std::f32::consts::LN_2;
    // e^f for |f| ≤ ln2/2, Taylor to the 6th power (error below 2e-7).
    let p = 1.0
        + f * (1.0
            + f * (1.0 / 2.0
                + f * (1.0 / 6.0 + f * (1.0 / 24.0 + f * (1.0 / 120.0 + f * (1.0 / 720.0))))));
    let scale = f32::from_bits(((whole as i32 + 127) as u32) << 23);
    p * scale
}

/// log2 of a positive, normal `x`.
#[inline]
pub fn log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let mut exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if mantissa > std::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    // ln(m) = 2·atanh(s), s = (m − 1)/(m + 1), |s| ≤ 0.172.
    let s = (mantissa - 1.0) / (mantissa + 1.0);
    let s2 = s * s;
    let ln = 2.0 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (1.0 / 5.0 + s2 * (1.0 / 7.0 + s2 / 9.0))));
    exponent as f32 + ln * std::f32::consts::LOG2_E
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_decibels_is_exactly_one_and_octaves_are_exact() {
        assert_eq!(decibels_to_level(0.0), 1.0);
        for k in -20..=20 {
            assert_eq!(exp2(k as f32), 2f32.powi(k));
        }
    }

    #[test]
    fn decibels_to_level_matches_powf_closely() {
        for i in -1200..=480 {
            let db = i as f32 * 0.1;
            let exact = 10f64.powf(f64::from(db) / 20.0);
            let fast = f64::from(decibels_to_level(db));
            assert!(
                ((fast - exact) / exact).abs() < 1e-6,
                "{db} dB: {fast} vs {exact}"
            );
        }
    }

    #[test]
    fn level_to_decibels_matches_log10_closely() {
        for i in 1..=4000 {
            let level = 10f32.powf(-6.0 + i as f32 * 0.0015);
            let exact = 20.0 * f64::from(level).log10();
            let fast = f64::from(level_to_decibels(level));
            assert!((fast - exact).abs() < 1e-4, "{level}: {fast} vs {exact}");
        }
    }

    #[test]
    fn silence_and_nonsense_read_as_the_floor() {
        for level in [0.0, 1e-9, f32::NAN, f32::INFINITY, -0.0] {
            assert_eq!(level_to_decibels(level), FLOOR_DECIBELS);
        }
        assert!((level_to_decibels(-0.5) - level_to_decibels(0.5)).abs() < 1e-9);
        assert_eq!(level_to_decibels(1.0), 0.0);
    }

    #[test]
    fn exp2_rises_across_its_rounding_boundaries() {
        for k in -20..20 {
            let half = k as f32 + 0.5;
            let below = exp2(half - 1e-5);
            let at = exp2(half);
            let above = exp2(half + 1e-5);
            assert!(below < at && at < above, "{half}: {below} {at} {above}");
        }
    }
}
