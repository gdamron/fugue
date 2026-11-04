//! Checks of the measurement helpers themselves, against reference noise
//! generated in this file, so a failure elsewhere points at the module and
//! not at the rig. These do not touch the `noise` module.

use super::rig::*;

fn reference_white(frames: usize) -> Vec<f32> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f64 / (1u64 << 52) as f64 - 1.0) as f32
        })
        .collect()
}

/// Voss-McCartney pink: `ROWS` independent white rows, row `k` redrawn
/// every `2^k` samples, summed. Each row is a sample-and-hold of white at
/// half the previous row's rate, so the sum's spectrum falls about 3 dB per
/// octave across the band the rows cover. Derived from that description; no
/// outside constants.
fn reference_pink(frames: usize) -> Vec<f32> {
    const ROWS: usize = 12;
    let white = reference_white(frames * 2);
    let mut draw = white.into_iter();
    let mut next = || f64::from(draw.next().expect("enough reference white"));
    let mut rows = [0.0f64; ROWS];
    for row in rows.iter_mut() {
        *row = next();
    }
    (0..frames)
        .map(|n| {
            // Row 0 changes every sample, row k every 2^k samples.
            for (k, row) in rows.iter_mut().enumerate() {
                if n % (1 << k) == 0 {
                    *row = next();
                }
            }
            (rows.iter().sum::<f64>() / ROWS as f64) as f32
        })
        .collect()
}

#[test]
fn reference_white_measures_as_white() {
    let white = reference_white(seconds(10.0));
    assert!(mean(&white).abs() < 0.01 && (rms(&white) - 0.577).abs() < 0.02);
    let slope = slope_db_per_octave(&third_octave_density_db(&welch_psd(&white)));
    assert!(slope.abs() < 0.5, "slope {slope}");
}

#[test]
fn reference_pink_measures_as_minus_three_db_per_octave() {
    let pink = reference_pink(seconds(10.0));
    let slope = slope_db_per_octave(&third_octave_density_db(&welch_psd(&pink)));
    assert!((slope + 3.0).abs() < 0.5, "slope {slope}");
}

#[test]
fn a_pure_tone_lands_in_its_bin() {
    let tone: Vec<f32> = (0..seconds(5.0))
        .map(|i| (2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / SRF).sin() as f32)
        .collect();
    let psd = welch_psd(&tone);
    let top = (0..psd.len())
        .max_by(|a, b| psd[*a].total_cmp(&psd[*b]))
        .unwrap();
    let hz = top as f64 * SRF / SEGMENT as f64;
    assert!((hz - 1_000.0).abs() < 2.0, "peak at {hz} Hz");
}
