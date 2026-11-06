//! A5: the `wavetable` output is band-limited.

use super::rig::*;

/// Window length: about 0.34 s, a power of two.
const WINDOW: usize = 1 << 14;
/// Bins either side of a harmonic that count as that harmonic (the
/// Blackman-Harris main lobe is +-4 bins).
const SPREAD: usize = 5;

/// Power at harmonics of `f0` over power at everything else between DC and
/// 20 kHz, in dB. DC and its main lobe are excluded.
pub fn harmonic_to_other_db(p: &[f64], f0: f64) -> f64 {
    let bin_hz = SRF / WINDOW as f64;
    let top = (20_000.0 / bin_hz) as usize;
    let harmonics: Vec<f64> = (1..)
        .map(|k| f64::from(k) * f0 / bin_hz)
        .take_while(|b| *b < top as f64 + SPREAD as f64)
        .collect();
    let (mut on, mut off) = (0.0, 0.0);
    for (b, v) in p.iter().enumerate().take(top + 1).skip(SPREAD + 1) {
        if harmonics
            .iter()
            .any(|h| (b as f64 - h).abs() <= SPREAD as f64)
        {
            on += v;
        } else {
            off += v;
        }
    }
    db(on / off.max(1e-300))
}

/// Criterion A5: "at a fundamental of 48000/23.5 Hz and 48000/11.5 Hz, at the
/// bright settings (timbre 1, morph 0 and morph 1, harmonics 0, 2/7 and 3/7),
/// the power of `audio` at non-harmonic frequencies within 0-20 kHz is at
/// least 30 dB below the power at its harmonics" (aux excluded). Measured
/// over a 16384-frame Blackman-Harris window after 0.3 s, harmonics counted
/// within +-5 bins.
#[test]
fn wavetable_is_band_limited_at_high_pitch_and_bright_settings() {
    let position = wavetable();
    for period in [23.5f64, 11.5] {
        let f0 = 48_000.0 / period;
        for harmonics in [0.0f32, 2.0 / 7.0, 3.0 / 7.0] {
            for morph in [0.0f32, 1.0] {
                let controls = [
                    ("frequency", f0 as f32),
                    ("harmonics", harmonics),
                    ("timbre", 1.0),
                    ("morph", morph),
                ];
                let mut v = settled(position, &controls);
                let x = v.render(WINDOW);
                assert!(peak(&x) > 0.01, "silent at {f0} Hz, harmonics {harmonics}");
                let margin = harmonic_to_other_db(&power_spectrum(&x, WINDOW), f0);
                assert!(
                    margin >= 30.0,
                    "{f0:.1} Hz, harmonics {harmonics:.3}, morph {morph}: non-harmonic \
                     power is only {margin:.1} dB below the harmonics"
                );
            }
        }
    }
}

/// Self-check of the measurement: a sum of sines at the harmonics of f0 reads
/// far above 30 dB, and a naive sawtooth (which aliases) reads below it.
#[test]
fn harmonic_measurement_separates_clean_from_aliased() {
    for period in [23.5f64, 11.5] {
        let f0 = 48_000.0 / period;
        let n = WINDOW;
        let clean: Vec<f32> = (0..n)
            .map(|i| {
                (1..=(23_000.0 / f0) as i32)
                    .map(|k| {
                        (std::f64::consts::TAU * f64::from(k) * f0 * i as f64 / SRF).sin()
                            / f64::from(k)
                    })
                    .sum::<f64>() as f32
                    * 0.3
            })
            .collect();
        let naive: Vec<f32> = (0..n)
            .map(|i| 0.5 * (2.0 * ((i as f64 / period).fract()) - 1.0) as f32)
            .collect();
        let clean_db = harmonic_to_other_db(&power_spectrum(&clean, n), f0);
        let naive_db = harmonic_to_other_db(&power_spectrum(&naive, n), f0);
        assert!(
            clean_db > 60.0,
            "clean series reads {clean_db} dB at {f0} Hz"
        );
        assert!(naive_db < 30.0, "naive saw reads {naive_db} dB at {f0} Hz");
    }
}
