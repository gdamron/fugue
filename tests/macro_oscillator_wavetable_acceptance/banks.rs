//! The bank layout described with the wavetable model: four banks, `timbre`
//! = 1 brightest, a gliding lower half and a stepped upper half of
//! `harmonics`.

use super::rig::*;

const WINDOW: usize = 1 << 13;

fn spectrum_of(controls: &[(&str, f32)]) -> Vec<f64> {
    let mut v = settled(wavetable(), controls);
    power_spectrum(&v.render(WINDOW), WINDOW)
}

fn macros(harmonics: f32, timbre: f32, morph: f32) -> Vec<(&'static str, f32)> {
    vec![
        ("frequency", 220.0),
        ("harmonics", harmonics),
        ("timbre", timbre),
        ("morph", morph),
    ]
}

/// Spec: "In every bank, `timbre` = 1 is the brightest column." Tested at the
/// bank centres (harmonics b/7) with morph 0.5, by spectral centroid.
/// Assumption: brightness is the power-weighted mean frequency, compared with
/// 2% slack against timbre 0, 0.25, 0.5 and 0.75 (points between columns);
/// the row (morph) is 0.5.
#[test]
fn timbre_one_is_the_brightest_column_in_every_bank() {
    for bank in 0..4 {
        let harmonics = bank as f32 / 7.0;
        let at = |timbre: f32| centroid(&spectrum_of(&macros(harmonics, timbre, 0.5)), WINDOW);
        let bright = at(1.0);
        for timbre in [0.0f32, 0.25, 0.5, 0.75] {
            let other = at(timbre);
            assert!(
                bright >= 0.98 * other,
                "bank {bank}: timbre 1 centroid {bright:.0} Hz is below timbre {timbre} \
                 ({other:.0} Hz)"
            );
        }
    }
}

/// Spec: four generated banks (harmonic spectra, formants, sync and pulse
/// width, bit-reduced shapes), bank b at harmonics = b/7. Tested only as
/// "the four bank centres sound clearly different": the octave-band spectra
/// of every pair differ by more than 1 dB on average.
/// Assumption: which bank is which is not tested, only that they differ.
#[test]
fn the_four_bank_centres_sound_different() {
    let bands: Vec<Vec<f64>> = (0..4)
        .map(|b| octave_bands_db(&spectrum_of(&macros(b as f32 / 7.0, 0.5, 0.5)), WINDOW))
        .collect();
    for a in 0..4 {
        for b in a + 1..4 {
            let d = band_distance(&bands[a], &bands[b]);
            assert!(d > 1.0, "banks {a} and {b} differ by only {d:.2} dB");
        }
    }
}

/// Fraction of adjacent `harmonics` steps (0.04 apart) over `values` whose
/// octave-band spectra are identical to within 0.02 dB.
fn plateau_share(values: &[f32]) -> f64 {
    let bands: Vec<Vec<f64>> = values
        .iter()
        .map(|h| octave_bands_db(&spectrum_of(&macros(*h, 0.5, 0.5)), WINDOW))
        .collect();
    let same = bands
        .windows(2)
        .filter(|w| band_distance(&w[0], &w[1]) < 0.02)
        .count();
    same as f64 / (bands.len() - 1) as f64
}

/// Spec: "The lower half of `harmonics` glides smoothly from bank to bank ...
/// The upper half steps back through the banks with stepped ('quantized')
/// scanning." Tested as: across the lower half the sound changes at
/// (nearly) every 0.04 step of `harmonics`, while across the upper half it
/// is constant over runs of steps (plateaus), at least 40% of adjacent steps
/// being identical.
/// Assumption: stepped means piecewise-constant spectra at fixed `timbre` and
/// `morph`; gliding means a different spectrum at neighbouring settings
/// (fewer than 40% of adjacent steps identical).
#[test]
fn lower_half_glides_and_upper_half_steps() {
    let lower: Vec<f32> = (0..13).map(|i| 0.04 * i as f32).collect();
    let upper: Vec<f32> = (0..13).map(|i| 0.52 + 0.04 * i as f32).collect();
    let (low, high) = (plateau_share(&lower), plateau_share(&upper));
    assert!(
        low < 0.4,
        "lower half has plateaus in {:.0}% of steps",
        low * 100.0
    );
    assert!(
        high >= 0.4,
        "upper half has plateaus in only {:.0}% of steps",
        high * 100.0
    );
}
