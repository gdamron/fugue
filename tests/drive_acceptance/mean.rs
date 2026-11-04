//! Criterion: "Asymmetric shapes: the output mean is below 1e-3 after 1 s."

use super::rig::*;
use super::*;

/// 240 Hz is exactly 200 samples per period at 48 kHz, so a 1 s window holds
/// whole periods and the sine's own ripple averages to zero.
const F: f64 = 240.0;

fn window_mean(config: Value, amp: f64) -> f64 {
    let mut rig = Rig::new(config);
    let s = sine(amp, F);
    rig.run(secs(1.0), &s);
    mean(&rig.run(secs(1.0), &s))
}

/// Shape `asymmetric`, across bias, drive, input level and auto level: the
/// mean of the second after the first stays below 1e-3 in magnitude.
#[test]
fn asymmetric_shape_has_near_zero_mean_after_one_second() {
    for bias in [-1.0, -0.5, 0.0, 0.5, 1.0] {
        for drive in [0.25, 1.0] {
            for amp in [0.1, 0.5, 0.9] {
                for auto in [true, false] {
                    let cfg = json!({ "shape": "asymmetric", "drive": drive, "bias": bias,
                        "auto_level": auto, "mix": 1.0, "level": 1.0 });
                    let m = window_mean(cfg, amp);
                    assert!(
                        m.abs() < 1e-3,
                        "mean {m:e} (bias {bias}, drive {drive}, amp {amp}, auto {auto})"
                    );
                }
            }
        }
    }
}

/// Measurement check (Assumption: asymmetric is asymmetric): the shape really
/// produces an even harmonic, otherwise the mean test would be vacuous.
/// The 2nd harmonic is at least 1% of the fundamental at drive 1, bias 0.5.
#[test]
fn asymmetric_shape_generates_even_harmonics() {
    let mut rig = Rig::new(json!({ "shape": "asymmetric", "drive": 1.0, "bias": 0.5 }));
    let s = sine(0.5, F);
    rig.run(secs(1.0), &s);
    let x = rig.run(secs(1.0), &s);
    let ratio = tone_amp(&x, 2.0 * F) / tone_amp(&x, F);
    assert!(ratio > 0.01, "2nd/1st harmonic ratio {ratio}");
}

/// Assumption (from the `bias` doc: a DC blocker removes the offset the bias
/// makes): any curve with bias ends up with a near-zero mean, not only the
/// asymmetric shape.
#[test]
fn biased_curves_have_near_zero_mean_after_one_second() {
    for shape in ["soft", "hard", "fold"] {
        for bias in [-0.5, 0.5] {
            let cfg = json!({ "shape": shape, "drive": 1.0, "bias": bias });
            let m = window_mean(cfg, 0.5);
            assert!(m.abs() < 1e-3, "{shape} bias {bias}: mean {m:e}");
        }
    }
}
