//! The `gain_curve` atom: a compressor's static curve in the log domain.

use super::rig::*;
use super::*;

/// The pinned design: the gain change in dB for an input level `x` dB.
fn design(x: f64, t: f64, r: f64, w: f64) -> f64 {
    let y = if 2.0 * (x - t) < -w {
        x
    } else if w > 0.0 && 2.0 * (x - t).abs() <= w {
        x + (1.0 / r - 1.0) * (x - t + w / 2.0).powi(2) / (2.0 * w)
    } else {
        t + (x - t) / r
    };
    y - x
}

/// The output for each steady input level, after two blocks of each.
fn steady(config: Value, levels: &[f32]) -> Vec<f32> {
    let mut curve = build("gain_curve", config);
    let at = |i: usize| levels[i];
    let out = curve.run(
        256 * levels.len(),
        &[("level", &move |t: usize| at(t / 256))],
    );
    (0..levels.len()).map(|i| out[0][256 * i + 255]).collect()
}

/// Criterion: "the static curve is within +/-0.1 dB of the design at
/// threshold -20, ratio 4 and knee 0 and 6, for inputs from -40 to 0 dBFS",
/// in 0.25 dB steps, plus one more setting; a negative level reads as its
/// magnitude.
#[test]
fn the_static_curve_matches_the_design() {
    let xs: Vec<f64> = (0..=160).map(|i| -40.0 + 0.25 * f64::from(i)).collect();
    for (t, r, w) in [(-20.0, 4.0, 0.0), (-20.0, 4.0, 6.0), (-10.0, 2.0, 12.0)] {
        for sign in [1.0, -1.0] {
            let levels: Vec<f32> = xs.iter().map(|x| (sign * db_to_amp(*x)) as f32).collect();
            let out = steady(json!({ "threshold": t, "ratio": r, "knee": w }), &levels);
            for (x, got) in xs.iter().zip(out) {
                let want = design(*x, t, r, w);
                let err = (f64::from(got) - want).abs();
                assert!(
                    err <= 0.1,
                    "T {t} R {r} W {w}, {x} dB (sign {sign}): {got}, want {want}"
                );
            }
        }
    }
}

/// Levels at or below 1e-6 read as -120 dB, and non-finite levels too: with
/// threshold -80 and a hard knee, 0 dB gain change, finite. A level just over
/// threshold still compresses (so the floor is not hiding everything).
#[test]
fn tiny_and_non_finite_levels_read_as_minus_120_db() {
    let config = json!({ "threshold": -80.0, "ratio": 50.0, "knee": 0.0 });
    let levels = [
        0.0,
        1e-9,
        1e-6,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1e-3,
    ];
    let out = steady(config, &levels);
    for (level, got) in levels.iter().zip(&out[..6]) {
        assert!(got.is_finite() && got.abs() < 1e-3, "level {level}: {got}");
    }
    assert!((f64::from(out[6]) - design(-60.0, -80.0, 50.0, 0.0)).abs() <= 0.1);
}
