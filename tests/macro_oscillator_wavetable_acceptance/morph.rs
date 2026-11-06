//! A4: click-free morphing of `harmonics`, `timbre` and `morph`.

use super::rig::*;

/// Largest step across a block edge (the step into the first sample of a
/// block and its two neighbours) and largest step elsewhere.
pub fn edge_and_other_steps(x: &[f32]) -> (f32, f32) {
    let (mut edge, mut other) = (0.0f32, 0.0f32);
    for n in 1..x.len() {
        let step = (x[n] - x[n - 1]).abs();
        let r = n % HOST_BLOCK;
        if r == 0 || r == 1 || r == HOST_BLOCK - 1 {
            edge = edge.max(step);
        } else {
            other = other.max(step);
        }
    }
    (edge, other)
}

/// Criterion A4: "a full-range sweep of `harmonics`, of `timbre` and of
/// `morph` (each over about 100 ms, written once per 64-frame host block, at
/// 220 Hz) shows no steps at host-block edges: the largest sample-to-sample
/// step next to a block edge is no more than 1.5x the largest step elsewhere
/// in the same sweep."
/// Assumptions: "next to a block edge" is the step into the first sample of a
/// block plus the steps just before and after it; the sweep is linear; both
/// directions and two settings of the other macros (0.5 and 0.25) are
/// tested.
#[test]
fn macro_sweeps_have_no_steps_at_block_edges() {
    let position = wavetable();
    for key in ["harmonics", "timbre", "morph"] {
        for others in [0.5f32, 0.25] {
            for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
                let mut controls = vec![("frequency", 220.0), ("harmonics", others)];
                controls.extend([("timbre", others), ("morph", others), (key, from)]);
                let mut v = settled(position, &controls);
                let x = v.sweep(key, from, to, seconds(0.1));
                assert!(peak(&x) > 0.01, "{key} sweep is silent");
                let (edge, other) = edge_and_other_steps(&x);
                assert!(
                    other > 0.0 && edge <= 1.5 * other,
                    "{key} {from} -> {to} (others {others}): largest step at block edges \
                     {edge} vs {other} elsewhere"
                );
            }
        }
    }
}

/// Self-check of the measurement: a clean sine passes, and a sine with a 0.2
/// jump spliced in at a block edge is flagged.
#[test]
fn edge_measurement_sees_a_spliced_jump() {
    let sine: Vec<f32> = (0..HOST_BLOCK * 20)
        .map(|i| 0.3 * (std::f64::consts::TAU * 220.0 * i as f64 / SRF).sin() as f32)
        .collect();
    let (edge, other) = edge_and_other_steps(&sine);
    assert!(edge <= 1.5 * other, "clean sine: {edge} vs {other}");
    let mut jumped = sine.clone();
    for v in &mut jumped[HOST_BLOCK * 7..] {
        *v += 0.2;
    }
    let (edge, other) = edge_and_other_steps(&jumped);
    assert!(edge > 1.5 * other, "jump at an edge: {edge} vs {other}");
}
