//! A6: `aux` is the wavetable signal reduced to 5-bit resolution.

use super::rig::*;
use std::collections::BTreeSet;

fn settings() -> Vec<Vec<(&'static str, f32)>> {
    vec![
        vec![],
        vec![("harmonics", 0.0), ("timbre", 1.0), ("morph", 0.0)],
        vec![("harmonics", 2.0 / 7.0), ("timbre", 0.5), ("morph", 1.0)],
        vec![("harmonics", 3.0 / 7.0), ("timbre", 0.25), ("morph", 0.5)],
        vec![("harmonics", 0.9), ("timbre", 0.8), ("morph", 0.3)],
        vec![("frequency", 110.0)],
        vec![("frequency", 1760.0)],
    ]
}

fn distinct(x: &[f32]) -> BTreeSet<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

/// Criterion A6: "in steady state with nothing patched and `level` 1, `aux`
/// takes at most 65 distinct values". Controls: `audio` takes many more
/// values and `aux` takes more than a few, so a silent or constant `aux` does
/// not pass.
#[test]
fn aux_takes_at_most_65_distinct_values() {
    let position = wavetable();
    for controls in settings() {
        let mut v = settled(position, &controls);
        v.set("level", 1.0);
        let (audio, aux) = v.render_both(seconds(0.5));
        let values = distinct(&aux);
        assert!(
            values.len() <= 65,
            "{controls:?}: aux took {} distinct values",
            values.len()
        );
        assert!(
            values.len() >= 4,
            "{controls:?}: aux took only {} distinct values (silent or constant?)",
            values.len()
        );
        assert!(
            distinct(&audio).len() > 1000,
            "{controls:?}: audio is itself coarse ({} values)",
            distinct(&audio).len()
        );
    }
}

/// Criterion A6 (stronger reading): "values are multiples of 1/32 before the
/// module's output level and gate", so after one fixed scale the distinct
/// values lie on an even grid. Test: every distinct value sits an integer
/// number of the smallest gap above the lowest one.
/// Assumption: with level 1 and the gate open the remaining scale is one
/// constant, so the grid is exact to float rounding (tolerance 1e-3 gap).
#[test]
fn aux_values_lie_on_an_even_grid() {
    let position = wavetable();
    for controls in settings() {
        let mut v = settled(position, &controls);
        v.set("level", 1.0);
        let (_, aux) = v.render_both(seconds(0.5));
        let mut values: Vec<f64> = distinct(&aux)
            .into_iter()
            .map(|b| f64::from(f32::from_bits(b)))
            .collect();
        values.sort_by(f64::total_cmp);
        let gap = values
            .windows(2)
            .map(|w| w[1] - w[0])
            .fold(f64::INFINITY, f64::min);
        assert!(gap.is_finite() && gap > 1e-6, "{controls:?}: gap {gap}");
        for value in &values {
            let steps = (value - values[0]) / gap;
            assert!(
                (steps - steps.round()).abs() < 1e-3,
                "{controls:?}: {value} is {steps} gaps above the lowest value"
            );
        }
    }
}
