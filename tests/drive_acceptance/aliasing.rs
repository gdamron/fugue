//! Criterion: "Hard clip at max drive on a 5 kHz sine: 4x oversampling cuts
//! aliased components by at least 30 dB versus none."

use super::rig::*;
use super::*;

const N: usize = 16_384;

/// Power in bins that are not within a few bins of a multiple of 5 kHz (the
/// true harmonics), skipping DC and the lowest 150 Hz. At 48 kHz every
/// harmonic of 5 kHz above Nyquist folds to an integer kHz that is not
/// necessarily a multiple of 5 kHz, so that is where aliasing shows.
fn aliased_power(x: &[f32], max_hz: f64) -> f64 {
    let hz = SRF / N as f64;
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

fn measure(oversampling: u32, amp: f64, max_hz: f64) -> (f64, f64) {
    let mut rig = Rig::new(json!({
        "shape": "hard", "drive": 1.0, "oversampling": oversampling,
        "mix": 1.0, "level": 1.0, "pre_tone": 0.0, "post_tone": 0.0, "bias": 0.0
    }));
    let s = sine(amp, 5_000.0);
    rig.run(secs(0.5), &s);
    let x = rig.run(N, &s);
    let fundamental = power_spectrum(&x)[(5_000.0 / (SRF / N as f64)).round() as usize];
    (aliased_power(&x, max_hz), fundamental)
}

fn cut_db(amp: f64, max_hz: f64) -> f64 {
    let (none, f1) = measure(1, amp, max_hz);
    let (four, f4) = measure(4, amp, max_hz);
    assert!(
        f1 > 0.0 && f4 > 0.0,
        "5 kHz fundamental present (amp {amp})"
    );
    let db = 10.0 * (none / four.max(1e-30)).log10();
    eprintln!("amp {amp}, below {max_hz} Hz: aliased energy cut {db:.1} dB by 4x");
    db
}

const NYQUIST: f64 = SRF / 2.0;
const AUDIBLE: f64 = 20_000.0;

/// The criterion read as written: a -6 dBFS (0.5 peak) sine, hard shape,
/// drive 1, over a Blackman-Harris FFT of 16384 points after 0.5 s of
/// settling; all non-harmonic bins up to Nyquist count.
#[test]
fn four_x_oversampling_cuts_aliasing_by_30_db_at_minus_6_dbfs() {
    assert!(cut_db(0.5, NYQUIST) >= 30.0);
}

/// The same with a near-full-scale (0.95 peak) sine.
#[test]
fn four_x_oversampling_cuts_aliasing_by_30_db_near_full_scale() {
    assert!(cut_db(0.95, NYQUIST) >= 30.0);
}

/// Narrower reading: only non-harmonic bins below 20 kHz count, leaving out
/// the decimation filter's transition band just under Nyquist (the 25 kHz
/// harmonic folds to 23 kHz there).
#[test]
fn four_x_oversampling_cuts_aliasing_below_20_khz_by_30_db() {
    assert!(cut_db(0.5, AUDIBLE) >= 30.0);
    assert!(cut_db(0.95, AUDIBLE) >= 30.0);
}

/// Measurement check: with no oversampling the hard clip does alias, so the
/// ratio above is measuring something (aliased energy within 40 dB of the
/// fundamental's).
#[test]
fn without_oversampling_hard_clip_visibly_aliases() {
    let (aliased, fundamental) = measure(1, 0.5, SRF);
    let rel = 10.0 * (aliased / fundamental).log10();
    assert!(
        rel > -40.0,
        "aliased energy only {rel:.1} dB below the fundamental"
    );
}
