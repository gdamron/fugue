//! Determinism: the same seed gives bit-identical output, and writing the
//! `seed` control restarts the random stream.

use crate::rig::*;

impl Rig {
    /// Every output sample of every block, as bit patterns.
    pub fn run_raw(&mut self, steps: usize) -> Vec<u32> {
        let mut bits = Vec::with_capacity(steps * BLOCK * OUTPUTS.len());
        for _ in 0..steps {
            self.block([1.0, 1.0, 0.0, 0.0], [0.0; BLOCK]);
            for at in 0..BLOCK {
                bits.extend(self.snapshot(at).bits());
            }
        }
        bits
    }
}

fn determinism_configs(seed: serde_json::Value) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({ "seed": seed }),
        serde_json::json!({ "seed": seed, "mode": "independent", "bias": 0.3, "jitter": 0.6 }),
        serde_json::json!({ "seed": seed, "mode": "divider", "bias": 0.4 }),
        serde_json::json!({ "seed": seed, "repetition": 0.5, "loop_length": 6, "steps": 0.2 }),
        serde_json::json!({ "seed": seed, "repetition": 1.0, "loop_length": 3, "steps": 0.9 }),
        serde_json::json!({ "seed": seed, "rate": "x2", "spread": 0.9, "key": "E", "scale": "blues" }),
    ]
}

#[test]
fn the_same_seed_gives_bit_identical_output_on_every_sample() {
    for seed in [
        serde_json::json!(0),
        serde_json::json!(1),
        serde_json::json!(1u64 << 40 | 7),
    ] {
        for config in determinism_configs(seed.clone()) {
            let first = rig(config.clone()).run_raw(3_000);
            let second = rig(config.clone()).run_raw(3_000);
            assert!(first == second, "same seed, different output for {config}");
            let nonconstant = first
                .windows(OUTPUTS.len())
                .any(|w| w != &first[..OUTPUTS.len()]);
            assert!(nonconstant, "output should not be constant for {config}");
        }
    }
}

#[test]
fn different_seeds_give_different_output() {
    for config in determinism_configs(serde_json::json!(1)) {
        let mut other = config.clone();
        other["seed"] = serde_json::json!(2);
        let a = rig(config.clone()).run_raw(2_000);
        let b = rig(other).run_raw(2_000);
        assert!(a != b, "seeds 1 and 2 should differ for {config}");
    }
}

#[test]
fn without_a_seed_each_module_draws_from_entropy() {
    let a = rig(serde_json::json!({ "spread": 1.0 })).run_raw(2_000);
    let b = rig(serde_json::json!({ "spread": 1.0 })).run_raw(2_000);
    assert!(a != b, "two unseeded modules should not agree");
}

#[test]
fn writing_the_seed_restarts_the_random_stream() {
    // Interface: "Writing it restarts the random stream." With repetition 0
    // and complementary gates nothing but the stream decides what follows.
    let mut r = rig(serde_json::json!({ "seed": 7, "spread": 1.0 }));
    let before = r.run(300);
    r.set("seed", fugue::ControlValue::Number(7.0));
    let after = r.run(300);
    for i in 0..300 {
        assert_eq!(
            before[i].every_step_bits(),
            after[i].every_step_bits(),
            "step {i} after rewriting the seed should replay the stream"
        );
    }
    r.set("seed", fugue::ControlValue::Number(8.0));
    let other = r.run(300);
    assert!(
        (0..300).any(|i| before[i].every_step_bits() != other[i].every_step_bits()),
        "a different seed should give a different stream"
    );
}
