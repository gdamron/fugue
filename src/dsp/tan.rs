//! A cheap `tan` that stays accurate up to just below a quarter turn.
//!
//! A filter's bilinear prewarp is `tan(π·cutoff/rate)`, which runs from 0
//! towards π/2 as the cutoff rises to Nyquist. A Taylor polynomial is cheap
//! but falls away from `tan` as the angle grows: one of degree 7 sets a
//! 20 kHz cutoff near 19 kHz at 48 kHz. This one is a [5/4] Padé
//! approximant on `[0, π/4]`, with `tan(x) = 1/tan(π/2 − x)` above that, so
//! its relative error stays near `f32` rounding across the whole range for
//! one division.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

/// `tan(x)` for `x` in `[0, π/2)`, to within a few parts in 10⁶.
///
/// Outside that range the result is not `tan`; callers clamp the angle (a
/// cutoff below Nyquist) first.
#[inline]
pub fn tan_quarter_turn(x: f32) -> f32 {
    let reflected = x > FRAC_PI_4;
    let y = if reflected { FRAC_PI_2 - x } else { x };
    let y2 = y * y;
    let numerator = y * (945.0 + y2 * (-105.0 + y2));
    let denominator = 945.0 + y2 * (-420.0 + 15.0 * y2);
    if reflected {
        denominator / numerator
    } else {
        numerator / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::tan_quarter_turn;
    use std::f64::consts::PI;

    #[test]
    fn follows_tan_up_to_just_below_nyquist() {
        let mut worst = 0.0f64;
        for step in 1..=20_000 {
            let x = (step as f64 / 20_000.0) * 0.49 * PI;
            let got = tan_quarter_turn(x as f32) as f64;
            worst = worst.max((got / x.tan() - 1.0).abs());
        }
        assert!(worst < 1.0e-5, "worst relative error {worst}");
    }

    #[test]
    fn a_20_khz_cutoff_stays_at_20_khz() {
        let rate = 48_000.0f64;
        let g = tan_quarter_turn((PI * 20_000.0 / rate) as f32) as f64;
        let heard = g.atan() * rate / PI;
        assert!((heard - 20_000.0).abs() < 0.5, "{heard}");
    }
}
