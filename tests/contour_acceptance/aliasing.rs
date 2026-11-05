//! Criterion 5: at about 1 kHz, total alias power from 0 to 20 kHz in
//! `contour.0` is at least 20 dB below that of a naive ramp of the same
//! waveform.
//!
//! `looping`, `audio` range, `shape` 0.5, `smoothness` 0.5, `slope` 0.05 and
//! 0.95, at 48000 / 47.5 Hz (period 47.5 samples, so aliases fall halfway
//! between harmonics). Power is summed over a 4-term Blackman-Harris spectrum
//! of 65536 samples, outside +-8 bins of every harmonic, 0 to 20 kHz. The naive
//! reference (rise from -1 to 1 over `slope` of the cycle, linear fall back)
//! gets the same treatment in the test. The fundamental must also match the
//! naive one within 1.5 dB, so silence or the wrong waveform cannot pass.

use super::rig::*;
use super::*;

const N: usize = 65_536;
const WARM: usize = 4096;

/// (alias power, fundamental power) of `x`, windowed.
fn alias_power(x: &[f32], f0: f64) -> (f64, f64) {
    let w = |i: usize| {
        let t = std::f64::consts::TAU * i as f64 / N as f64;
        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
    };
    let mut re: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| f64::from(*v) * w(i))
        .collect();
    let mut im = vec![0.0; N];
    fft(&mut re, &mut im);
    let bin = SRF / N as f64;
    let (mut alias, mut fundamental) = (0.0, 0.0);
    for k in 0..=(20_000.0 / bin) as usize {
        let hz = k as f64 * bin;
        let power = re[k] * re[k] + im[k] * im[k];
        let off = (hz - (hz / f0).round() * f0).abs();
        if (hz - f0).abs() <= 8.0 * bin {
            fundamental += power;
        } else if off > 8.0 * bin {
            alias += power;
        }
    }
    (alias, fundamental)
}

fn naive(f0: f64, a: f64) -> Vec<f32> {
    (0..N)
        .map(|n| {
            let p = (n as f64 * f0 / SRF).fract();
            (if p < a {
                -1.0 + 2.0 * p / a
            } else {
                1.0 - 2.0 * (p - a) / (1.0 - a)
            }) as f32
        })
        .collect()
}

#[test]
fn band_limited_ramp_aliases_20_db_below_a_naive_ramp() {
    let f0 = SRF / 47.5;
    for slope in [0.05f32, 0.95] {
        let mut c = contour(json!({ "mode": "looping", "range": "audio", "shape": 0.5,
                                    "smoothness": 0.5, "slope": slope, "frequency": f0 }));
        let x = c.render(WARM + N, &[]);
        let (alias, fundamental) = alias_power(&x[0][WARM..], f0);
        let (naive_alias, naive_fundamental) = alias_power(&naive(f0, f64::from(slope)), f0);
        let db = |a: f64, b: f64| 10.0 * (a / b).log10();
        let level = db(fundamental, naive_fundamental);
        assert!(
            level.abs() <= 1.5,
            "slope {slope}: fundamental is {level:.2} dB off the naive ramp"
        );
        let below = db(alias, naive_alias);
        assert!(
            below <= -20.0,
            "slope {slope}: alias power only {below:.1} dB under naive"
        );
    }
}
