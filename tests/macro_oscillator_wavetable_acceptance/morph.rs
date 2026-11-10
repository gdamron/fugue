//! The block-edge step measure used by the chords morph sweep.

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
