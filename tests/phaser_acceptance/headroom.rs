//! Criterion 2: at maximum feedback (either sign) a 0 dBFS input stays
//! within +/-1.0, for the smallest and largest stage counts.

use super::rig::*;
use super::*;

fn assert_within_full_scale(config: Value, x: &[f32], what: &str) {
    let mut unit = build(config.clone());
    let (l, r) = unit.run_mono(x);
    for (side, out) in [("left", &l), ("right", &r)] {
        assert_finite(out, side);
        let top = peak(out);
        assert!(top <= 1.0, "{what} {config}: {side} peaks at {top}");
        assert!(top > 0.5, "{what} {config}: {side} is not silent ({top})");
    }
}

fn full_scale_sources(len: usize) -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("noise at 0 dBFS", noise(len, 1.0, 11)),
        ("square at 0 dBFS", square(len, 480, 1.0)),
        ("sine at 0 dBFS", sine(len, 997.0, 1.0)),
    ]
}

/// Four seconds of each source, with the sweep running (default and fast,
/// shallow and full depth) and at `mix` 0.5 (the default) and 1 (the chain
/// alone, the most the wet path can give).
#[test]
fn full_scale_input_stays_within_one_at_maximum_feedback() {
    let len = secs(4.0);
    for (name, x) in full_scale_sources(len) {
        for feedback in [0.95, -0.95] {
            for stage_count in [4, 12] {
                for mix in [0.5, 1.0] {
                    for (rate, depth) in [(0.5, 0.6), (10.0, 1.0)] {
                        let config = json!({ "feedback": feedback, "stage_count": stage_count,
                            "mix": mix, "rate": rate, "depth": depth });
                        assert_within_full_scale(config, &x, name);
                    }
                }
            }
        }
    }
}

/// The same at every stage count between, at the default sweep.
#[test]
fn full_scale_noise_stays_within_one_at_every_stage_count() {
    let x = noise(secs(3.0), 1.0, 5);
    for feedback in [0.95, -0.95] {
        for stage_count in 4..=12 {
            let config = json!({ "feedback": feedback, "stage_count": stage_count, "mix": 1.0 });
            assert_within_full_scale(config, &x, "noise");
        }
    }
}

/// Feedback raised from 0 to its maximum while 0 dBFS noise plays.
#[test]
fn raising_feedback_to_its_maximum_live_stays_within_one() {
    let x = noise(secs(3.0), 1.0, 9);
    for target in [0.95, -0.95] {
        for stage_count in [4, 12] {
            let mut unit =
                build(json!({ "feedback": 0.0, "stage_count": stage_count, "mix": 1.0 }));
            let (l, r) = unit.run_hooked(&x, |t, u| {
                if t == secs(1.0) {
                    u.set("feedback", target);
                }
            });
            for out in [&l, &r] {
                assert_finite(out, "output");
                assert!(
                    peak(out) <= 1.0,
                    "to {target}, {stage_count} stages: {}",
                    peak(out)
                );
            }
        }
    }
}
