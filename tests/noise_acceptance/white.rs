//! White output statistics at level 1.

use super::rig::*;
use super::*;

/// Criterion: the white statistics hold with the default (no `seed`) config
/// too, since `seed` is optional.
#[test]
fn white_statistics_hold_without_a_seed() {
    let (white, _) = noise(json!({})).render(seconds(10.0));
    assert!(mean(&white).abs() <= 0.01, "mean {}", mean(&white));
    assert!((rms(&white) - 0.577).abs() <= 0.02, "rms {}", rms(&white));
}

/// Criterion (white, shape): white noise is white. The spec gives no
/// spectrum criterion for white, but it names the output "white": consecutive
/// samples must be uncorrelated and the spectrum flat to within 1.5 dB per
/// octave of slope, so a noise that merely matches mean and RMS (a constant,
/// a square wave) fails.
/// Assumption: flat means a fitted slope under 1 dB/oct, 50 Hz to 10 kHz.
#[test]
fn white_is_uncorrelated_and_spectrally_flat() {
    let (white, _) = seeded(7).render(seconds(10.0));
    let m = mean(&white);
    let num: f64 = white
        .windows(2)
        .map(|w| (f64::from(w[0]) - m) * (f64::from(w[1]) - m))
        .sum();
    let den: f64 = white.iter().map(|v| (f64::from(*v) - m).powi(2)).sum();
    let lag1 = num / den;
    assert!(lag1.abs() < 0.01, "lag-1 autocorrelation {lag1}");
    let slope = slope_db_per_octave(&third_octave_density_db(&welch_psd(&white)));
    assert!(slope.abs() < 1.0, "white spectral slope {slope} dB/oct");
}

/// Criterion: white is a genuine noise stream, not a short loop. No exact
/// repeat of the first 4096 samples appears in the next 10 s.
/// Assumption: a period under 10 s would be a defect for a noise source.
#[test]
fn white_does_not_loop_within_ten_seconds() {
    let (white, _) = seeded(7).render(seconds(10.0));
    let head = &white[..4096];
    let repeats = (4096..white.len() - 4096).filter(|&i| white[i..i + 4096] == *head);
    assert_eq!(repeats.count(), 0, "first 4096 samples recur");
}
