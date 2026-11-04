//! Spread: the spread of melody values grows with the `spread` control.

use crate::rig::*;

fn mean(values: &[f32]) -> f64 {
    values.iter().map(|v| *v as f64).sum::<f64>() / values.len() as f64
}

fn std_dev(values: &[f32]) -> f64 {
    let m = mean(values);
    (values.iter().map(|v| (*v as f64 - m).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

// ---------------------------------------------------------------------------
// Criterion 2: the spread of melody values grows monotonically with spread
// ---------------------------------------------------------------------------

#[test]
fn melody_value_spread_grows_monotonically_with_the_spread_control() {
    // Channel 3 takes a new value on every step. `steps` 0.4 is the
    // stepped, unquantized band, so value_3 is the raw random value.
    let spreads = [0.0f64, 0.2, 0.4, 0.6, 0.8, 1.0];
    let mut deviations = Vec::new();
    for spread in spreads {
        let mut r = rig(serde_json::json!({
            "spread": spread, "steps": 0.4, "value_bias": 0.5, "seed": 2024
        }));
        let values: Vec<f32> = r.run(20_000).iter().map(|s| s.value[2]).collect();
        assert!(
            values.iter().all(|v| (0.0..=1.0).contains(v)),
            "spread {spread}: values must stay within 0..1"
        );
        deviations.push(std_dev(&values));
    }
    for pair in deviations.windows(2).enumerate() {
        let (i, w) = pair;
        assert!(
            w[1] > w[0],
            "std dev must grow with spread: spread {} -> {}, std dev {} -> {} (all: {deviations:?})",
            spreads[i],
            spreads[i + 1],
            w[0],
            w[1]
        );
    }
}

#[test]
fn spread_zero_pins_values_to_value_bias_and_spread_one_is_uniform() {
    // Interface: "0 = always value_bias; 1 = uniform over 0-1."
    for value_bias in [0.2f64, 0.5, 0.8] {
        let mut r = rig(serde_json::json!({
            "spread": 0.0, "steps": 0.4, "value_bias": value_bias, "seed": 5
        }));
        for (i, s) in r.run(500).iter().enumerate() {
            assert!(
                (s.value[2] as f64 - value_bias).abs() < 1e-3,
                "step {i}: value {} should be value_bias {value_bias}",
                s.value[2]
            );
        }
    }
    let mut r = rig(serde_json::json!({ "spread": 1.0, "steps": 0.4, "seed": 5 }));
    let values: Vec<f32> = r.run(20_000).iter().map(|s| s.value[2]).collect();
    let uniform_sd = (1.0f64 / 12.0).sqrt();
    assert!((mean(&values) - 0.5).abs() < 0.02, "mean {}", mean(&values));
    assert!(
        (std_dev(&values) - uniform_sd).abs() < 0.02,
        "std dev {}",
        std_dev(&values)
    );
    let low = values.iter().filter(|v| **v < 0.1).count();
    let high = values.iter().filter(|v| **v > 0.9).count();
    assert!(
        low > 1_000 && high > 1_000,
        "uniform reaches the ends: {low} low, {high} high"
    );
}
