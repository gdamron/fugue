//! Criterion 1: a trigger's hit decays away.

use super::rig::*;

const PITCHES: [f32; 4] = [55.0, 110.0, 220.0, 440.0];
const RINGER_PITCHES: [f32; 3] = [55.0, 220.0, 880.0];

/// One hit from silence at `hz`, default decay (`morph` 0.5, every other
/// control at default, nothing patched but `trigger`), rendered for `frames`.
fn hit(name: &str, hz: f32, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut v = engine(name);
    v.set("frequency", hz);
    v.render_driven(frames, pulses(&[T0]))
}

fn assert_below(name: &str, hz: f32, port: &str, x: &[f32], from: usize, to: usize) {
    let p = peak(&x[from..to]);
    assert!(
        p < MINUS_90_DB,
        "{name} at {hz} Hz ({port}): peak {p:e} ({:.1} dBFS) in {:.1}-{:.1} s is not below -90 dBFS",
        20.0 * p.max(1e-12).log10(),
        from as f64 / SRF,
        to as f64 / SRF
    );
}

/// Criterion: "each percussive engine (bass_drum, snare_drum, hi_hat) falls
/// below -90 dBFS within 4 s of a trigger at default decay", at 55, 110, 220
/// and 440 Hz, on `audio` and on `aux`.
/// Assumptions: "falls below -90 dBFS" means the sample peak (stricter than
/// RMS) stays under 10^(-90/20) from 4.0 s to 4.5 s after the trigger; and
/// the hit was audible (`audio` peak above 0.01 in the first second), so a
/// dead engine cannot pass.
mod drums_fall_below_minus_90_db_within_4_s {
    use super::*;

    fn check(name: &str) {
        for hz in PITCHES {
            let (audio, aux) = hit(name, hz, seconds(4.5));
            let first = peak(&audio[..seconds(1.0)]);
            assert!(first > 0.01, "{name} at {hz} Hz: silent hit (peak {first})");
            assert_below(name, hz, "audio", &audio, seconds(4.0), seconds(4.5));
            assert_below(name, hz, "aux", &aux, seconds(4.0), seconds(4.5));
        }
    }

    each_drum!(check);
}

/// Criterion (clarified): the string and modal ring longer than the drums at
/// default morph by design, so a single trigger must eventually decay: below
/// -90 dBFS within 20 s on `audio` and `aux` (checked over 20.0-21.0 s), with
/// no growth.
/// Assumption: "no growth" means each 1 s window's peak is at most 1.1x the
/// peak of the window 5 s earlier (plus the -90 dBFS floor), for the whole
/// 21 s, and no window after the first exceeds the first second's peak.
mod ringers_decay_within_20_s_without_growth {
    use super::*;

    fn no_growth(name: &str, hz: f32, port: &str, x: &[f32]) {
        let w = windowed_peaks(x, seconds(1.0), seconds(1.0));
        for k in 5..w.len() {
            assert!(
                w[k] <= 1.1 * w[k - 5] + MINUS_90_DB,
                "{name} at {hz} Hz ({port}): second {k} peaks at {} after {} five seconds earlier",
                w[k],
                w[k - 5]
            );
        }
        for (k, p) in w.iter().enumerate().skip(1) {
            assert!(
                *p <= w[0] * 1.05 + MINUS_90_DB,
                "{name} at {hz} Hz ({port}): second {k} peaks at {p}, above the first second's {}",
                w[0]
            );
        }
    }

    fn check(name: &str) {
        for hz in RINGER_PITCHES {
            let (audio, aux) = hit(name, hz, seconds(21.0));
            let first = peak(&audio[..seconds(1.0)]);
            assert!(first > 0.01, "{name} at {hz} Hz: silent hit (peak {first})");
            assert_below(name, hz, "audio", &audio, seconds(20.0), seconds(21.0));
            assert_below(name, hz, "aux", &aux, seconds(20.0), seconds(21.0));
            no_growth(name, hz, "audio", &audio);
            no_growth(name, hz, "aux", &aux);
        }
    }

    each_ringer!(check);
}
