//! Pink output spectral slope.

use super::rig::*;

fn pink_bands(seed: u64) -> Vec<(f64, f64)> {
    let (_, pink) = seeded(seed).render(seconds(10.0));
    third_octave_density_db(&welch_psd(&pink))
}

/// Criterion: "Pink noise slopes at -3 dB/oct +/- 1 dB from 50 Hz to 10 kHz",
/// measured as the least-squares slope of the Welch-averaged power density
/// in third-octave bands, for several seeds.
#[test]
fn pink_slopes_minus_three_db_per_octave() {
    for seed in [1u64, 7, 99] {
        let slope = slope_db_per_octave(&pink_bands(seed));
        assert!(
            (slope + 3.0).abs() <= 1.0,
            "seed {seed}: pink slope {slope} dB/oct, want -3 +/- 1"
        );
    }
}

/// Criterion: the -3 dB/oct slope is a property of the whole 50 Hz..10 kHz
/// range, not an average of a bend. Each half (50 Hz..~700 Hz and
/// ~700 Hz..10 kHz) must itself slope -3 dB/oct.
/// Assumption: halves are allowed +/- 1.5 dB, a little looser than the full
/// range, since fewer bands average out less ripple.
#[test]
fn pink_slope_holds_in_each_half_of_the_range() {
    let bands = pink_bands(7);
    let (low, high) = bands.split_at(bands.len() / 2);
    for (name, half) in [("low", low), ("high", high)] {
        let slope = slope_db_per_octave(half);
        assert!(
            (slope + 3.0).abs() <= 1.5,
            "{name} half pink slope {slope} dB/oct"
        );
    }
}

/// Criterion: pink is a different signal from white, with far more
/// low-frequency than high-frequency power (at least 15 dB between the 50 Hz
/// and 10 kHz bands; an ideal -3 dB/oct slope gives about 22 dB).
#[test]
fn pink_is_not_the_white_output() {
    let (white, pink) = seeded(7).render(seconds(10.0));
    assert_ne!(white, pink);
    let bands = third_octave_density_db(&welch_psd(&pink));
    let (first, last) = (bands.first().unwrap().1, bands.last().unwrap().1);
    assert!(
        first - last > 15.0,
        "pink falls from {first} dB to {last} dB across the range"
    );
}
