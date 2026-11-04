//! Criterion: "Hard clip at max drive on a 5 kHz sine: 4x oversampling cuts
//! aliased components by at least 30 dB versus none." Measured over the
//! audible band, 0 to 20 kHz, at 48 kHz and at 44.1 kHz.

use super::rig::*;
use super::*;

const N: usize = 16_384;
const AUDIBLE: f64 = 20_000.0;

/// Power in bins below `max_hz` that are not within a few bins of a multiple
/// of 5 kHz (the true harmonics), skipping DC and the lowest 150 Hz. Every
/// harmonic of 5 kHz above Nyquist folds back to some other frequency, so
/// whatever is left between the harmonics is aliasing.
fn aliased_power(x: &[f32], sample_rate: f64, max_hz: f64) -> f64 {
    let hz = sample_rate / N as f64;
    power_spectrum(x)
        .iter()
        .enumerate()
        .filter(|(k, _)| {
            let f = *k as f64 * hz;
            let off = (f / 5_000.0 - (f / 5_000.0).round()).abs() * 5_000.0;
            f > 150.0 && f < max_hz && off > 6.0 * hz
        })
        .map(|(_, p)| p)
        .sum()
}

/// Hard shape, drive 1, a 5 kHz sine of peak `amp`, over a Blackman-Harris
/// FFT of 16384 points after 0.5 s of settling. Returns (aliased power,
/// fundamental power).
fn measure(sample_rate: u32, oversampling: u32, amp: f64, max_hz: f64) -> (f64, f64) {
    let sr = f64::from(sample_rate);
    let mut rig = Rig::at_rate(
        json!({
            "shape": "hard", "drive": 1.0, "oversampling": oversampling,
            "mix": 1.0, "level": 1.0, "pre_tone": 0.0, "post_tone": 0.0, "bias": 0.0
        }),
        sample_rate,
    );
    let s = sine_at(amp, 5_000.0, sr);
    rig.run((0.5 * sr) as usize, &s);
    let x = rig.run(N, &s);
    let fundamental = power_spectrum(&x)[(5_000.0 / (sr / N as f64)).round() as usize];
    (aliased_power(&x, sr, max_hz), fundamental)
}

/// Measured cut in dB of aliased energy below 20 kHz by 4x oversampling.
fn cut_db(sample_rate: u32, amp: f64) -> f64 {
    let (none, f1) = measure(sample_rate, 1, amp, AUDIBLE);
    let (four, f4) = measure(sample_rate, 4, amp, AUDIBLE);
    assert!(
        f1 > 0.0 && f4 > 0.0,
        "5 kHz fundamental present (amp {amp})"
    );
    let db = 10.0 * (none / four.max(1e-30)).log10();
    eprintln!("{sample_rate} Hz, amp {amp}: aliased energy below 20 kHz cut {db:.1} dB by 4x");
    db
}

fn assert_cut(sample_rate: u32, amp: f64) {
    let db = cut_db(sample_rate, amp);
    assert!(
        db >= 30.0,
        "{sample_rate} Hz, amp {amp}: aliased energy cut only {db:.1} dB (need 30)"
    );
}

/// 48 kHz, a -6 dBFS (0.5 peak) sine.
#[test]
fn four_x_oversampling_cuts_aliasing_below_20_khz_by_30_db_at_48k() {
    assert_cut(48_000, 0.5);
}

/// 48 kHz, a near-full-scale (0.95 peak) sine.
#[test]
fn four_x_oversampling_cuts_aliasing_below_20_khz_by_30_db_at_48k_near_full_scale() {
    assert_cut(48_000, 0.95);
}

/// 44.1 kHz, where folded harmonics land in 18 to 20 kHz: a -6 dBFS sine.
#[test]
fn four_x_oversampling_cuts_aliasing_below_20_khz_by_30_db_at_44_1k() {
    assert_cut(44_100, 0.5);
}

/// 44.1 kHz, near full scale (0.95 peak).
#[test]
fn four_x_oversampling_cuts_aliasing_below_20_khz_by_30_db_at_44_1k_near_full_scale() {
    assert_cut(44_100, 0.95);
}

/// Measurement check: with no oversampling the hard clip does alias, so the
/// ratio above is measuring something (aliased energy within 40 dB of the
/// fundamental's, over the whole band up to Nyquist).
#[test]
fn without_oversampling_hard_clip_visibly_aliases() {
    let (aliased, fundamental) = measure(48_000, 1, 0.5, SRF);
    let rel = 10.0 * (aliased / fundamental).log10();
    assert!(
        rel > -40.0,
        "aliased energy only {rel:.1} dB below the fundamental"
    );
}
