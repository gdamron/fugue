//! Repetition: a locked loop repeats with period exactly `loop_length`, and
//! with no repetition no window repeats.

use crate::rig::*;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Criterion 3: at repetition max with loop length N, gates and values repeat
// with period exactly N
// ---------------------------------------------------------------------------

#[test]
fn locked_repetition_repeats_with_period_exactly_the_loop_length() {
    for seed in [8u64, 909] {
        for n in [1usize, 2, 3, 5, 8, 13, 16] {
            let mut r = rig(serde_json::json!({
                "repetition": 1.0, "loop_length": n, "seed": seed,
                "mode": "independent", "bias": 0.5, "steps": 0.4
            }));
            let s = r.run(12 * n.max(4));

            // The outputs that refresh on every step repeat from the start.
            for i in 0..s.len() - n {
                assert_eq!(
                    s[i].every_step_bits(),
                    s[i + n].every_step_bits(),
                    "seed {seed}, loop {n}: step {i} and step {} differ",
                    i + n
                );
            }
            // Held channels (1 and 2) are settled after the first pass.
            for i in n..s.len() - n {
                assert_eq!(
                    s[i].bits(),
                    s[i + n].bits(),
                    "seed {seed}, loop {n}: after the first pass, step {i} and step {} differ",
                    i + n
                );
            }
            // Exactly N: no shorter period fits.
            for d in 1..n {
                let periodic = (n..s.len() - d).all(|i| s[i].bits() == s[i + d].bits());
                assert!(!periodic, "seed {seed}, loop {n}: also periodic with {d}");
            }
        }
    }
}

#[test]
fn locked_repetition_repeats_in_the_quantized_default_setup() {
    // Same property with the melody quantized (the default `steps`).
    let mut r = rig(serde_json::json!({ "repetition": 1.0, "loop_length": 8, "seed": 12 }));
    let s = r.run(200);
    for i in 8..s.len() - 8 {
        assert_eq!(s[i].bits(), s[i + 8].bits(), "step {i}");
    }
}

// ---------------------------------------------------------------------------
// Criterion 4: at repetition 0, no window of 2N steps repeats over 1,000 steps
// ---------------------------------------------------------------------------

#[test]
fn no_repetition_means_no_window_of_twice_the_loop_length_repeats() {
    // A window is the full tuple of outputs for 2N consecutive steps, so
    // channel 3's continuous value tells windows apart (gates alone are
    // binary and would collide by chance at these lengths).
    for seed in [41u64, 4242] {
        for n in [1usize, 4, 8, 16] {
            let mut r = rig(serde_json::json!({
                "repetition": 0.0, "loop_length": n, "seed": seed
            }));
            let s = r.run(1_000);
            let width = 2 * n;
            let mut seen: HashSet<Vec<[u32; 9]>> = HashSet::new();
            for i in 0..=s.len() - width {
                let window: Vec<[u32; 9]> = s[i..i + width].iter().map(Step::bits).collect();
                assert!(
                    seen.insert(window),
                    "seed {seed}, loop {n}: the {width}-step window at {i} repeats an earlier one"
                );
            }
        }
    }
}

#[test]
fn no_repetition_does_not_replay_the_loop_length_back() {
    // Weaker, gate-level companion: with repetition 0, how often step i+N
    // matches step i is chance level (complementary gate_1: about half),
    // not a replay.
    let n = 8;
    let mut r = rig(serde_json::json!({ "repetition": 0.0, "loop_length": n, "seed": 6 }));
    let s = r.run(2_000);
    let matches = (0..s.len() - n)
        .filter(|&i| s[i].fired(0) == s[i + n].fired(0))
        .count() as f64
        / (s.len() - n) as f64;
    assert!(
        (matches - 0.5).abs() < 0.05,
        "gate match rate at lag {n} was {matches}"
    );
}
