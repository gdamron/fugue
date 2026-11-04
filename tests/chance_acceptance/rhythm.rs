//! Rhythm: the gate rate follows `bias` in each mode, the divider plays a
//! metrical bar, and a reset edge returns it to the first step.

use crate::rig::*;

fn gate_rate(steps: &[Step], gate: usize) -> f64 {
    steps.iter().filter(|s| s.fired(gate)).count() as f64 / steps.len() as f64
}

// ---------------------------------------------------------------------------
// Criterion 1: over 10,000 clocks the gate rate equals bias +/- 0.02 at
// bias 0.1, 0.5 and 0.9 (default mode)
// ---------------------------------------------------------------------------

#[test]
fn gate_rate_equals_bias_within_two_percent_over_ten_thousand_clocks() {
    for seed in [3u64, 1234, 987_654_321] {
        for bias in [0.1f64, 0.5, 0.9] {
            let mut r = rig(serde_json::json!({ "bias": bias, "seed": seed }));
            let steps = r.run(10_000);
            let rate = gate_rate(&steps, 0);
            assert!(
                (rate - bias).abs() <= 0.02,
                "seed {seed}, bias {bias}: gate_1 rate was {rate}"
            );
            // Default mode is complementary: gate_2 fires exactly when gate_1 does not.
            for (i, s) in steps.iter().enumerate() {
                assert_ne!(
                    s.fired(0),
                    s.fired(1),
                    "step {i}: complementary gates must be exact opposites"
                );
            }
        }
    }
}

#[test]
fn independent_mode_tosses_each_gate_separately_at_bias() {
    // Interface: "Independent: two tosses, each gate fires with probability bias."
    for bias in [0.1f64, 0.5, 0.9] {
        let mut r = rig(serde_json::json!({
            "mode": "independent", "bias": bias, "seed": 77
        }));
        let steps = r.run(10_000);
        for gate in 0..2 {
            let rate = gate_rate(&steps, gate);
            assert!(
                (rate - bias).abs() <= 0.02,
                "bias {bias}: gate_{} rate was {rate}",
                gate + 1
            );
        }
    }
    let mut r = rig(serde_json::json!({ "mode": "independent", "bias": 0.5, "seed": 77 }));
    let steps = r.run(2_000);
    assert!(
        steps.iter().any(|s| s.fired(0) && s.fired(1)),
        "both gates can fire together"
    );
    assert!(
        steps.iter().any(|s| !s.fired(0) && !s.fired(1)),
        "neither gate can fire"
    );
}

// ---------------------------------------------------------------------------
// Supplementary: reset and divider (from the interface text)
// ---------------------------------------------------------------------------

#[test]
fn divider_mode_plays_a_sixteen_step_pattern_that_starts_on_a_downbeat() {
    // Interface: "drum-like metrical patterns over 16 steps (downbeats always
    // fire on gate_1, backbeats on gate_2; bias adds weaker positions)".
    // At bias 0 only the fixed positions remain, so the bar is deterministic.
    let mut r = rig(serde_json::json!({ "mode": "divider", "bias": 0.0, "seed": 1 }));
    let s = r.run(16 * 6);
    assert!(s[0].fired(0), "step 0 is a downbeat and fires gate_1");
    for bar in 1..6 {
        for i in 0..16 {
            assert_eq!(
                s[i].every_step_bits()[..2],
                s[bar * 16 + i].every_step_bits()[..2],
                "bar {bar} step {i} should match the first bar"
            );
        }
    }
    assert!(s[..16].iter().filter(|x| x.fired(0)).count() >= 1);
    assert!(
        s[..16].iter().filter(|x| x.fired(1)).count() >= 1,
        "backbeats fire gate_2"
    );
    assert!(
        s[..16].iter().all(|x| !(x.fired(0) && x.fired(1))),
        "a position is a downbeat or a backbeat, not both"
    );

    // Bias adds weaker positions but never removes the downbeat.
    for bias in [0.5, 1.0] {
        let mut r = rig(serde_json::json!({ "mode": "divider", "bias": bias, "seed": 1 }));
        let s = r.run(16 * 10);
        for bar in 0..10 {
            assert!(
                s[bar * 16].fired(0),
                "bias {bias}, bar {bar}: downbeat must fire"
            );
        }
    }
}

#[test]
fn a_reset_edge_returns_the_pattern_to_its_first_step() {
    let mut r = rig(serde_json::json!({ "mode": "divider", "bias": 0.0, "seed": 1 }));
    let first_bar = r.run(16);
    r.run(5); // now partway through the second bar
    r.reset_pulse();
    let after_reset = r.run(16);
    for i in 0..16 {
        assert_eq!(
            first_bar[i].every_step_bits()[..2],
            after_reset[i].every_step_bits()[..2],
            "step {i} after reset should replay the first bar"
        );
    }
}
