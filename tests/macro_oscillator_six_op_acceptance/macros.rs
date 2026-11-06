//! C7: click-free `timbre`, `morph` and patch changes.

use super::rig::*;

/// Largest step across a block edge (the step into the first sample of a
/// block and its two neighbours) and largest step elsewhere.
fn edge_and_other_steps(x: &[f32]) -> (f32, f32) {
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

/// Criterion C7 (first part): "droning at 220 Hz, full-range sweeps of
/// `timbre` and of `morph` (each over about 100 ms, written once per 64-frame
/// host block) show no steps at host-block edges, by A4's measure, on
/// patches 0, 9, 18 and 26." A4's measure: the largest step next to a block
/// edge is no more than 1.5x the largest step elsewhere in the same sweep.
/// Assumptions: "next to a block edge" is the step into the first sample of a
/// block plus the steps just before and after it; the sweep is linear; both
/// directions are tested; the other macros stay at 0.5.
#[test]
fn timbre_and_morph_sweeps_have_no_steps_at_block_edges() {
    for patch in [0usize, 9, 18, 26] {
        for key in ["timbre", "morph"] {
            for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
                let mut v = drone(patch, &[("timbre", 0.5), ("morph", 0.5), (key, from)]);
                let x = v.sweep(key, from, to, seconds(0.1));
                assert!(
                    peak(&x) > 0.001,
                    "{} {key} sweep is silent",
                    PATCH_NAMES[patch]
                );
                let (edge, other) = edge_and_other_steps(&x);
                assert!(
                    other > 0.0 && edge <= 1.5 * other,
                    "{} {key} {from} -> {to}: largest step at block edges {edge} vs \
                     {other} elsewhere",
                    PATCH_NAMES[patch]
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

/// Criterion C7 (second part): "A `harmonics` sweep from patch 8 to patch 12
/// over 400 ms has no sample-to-sample step larger than 1.5x the largest
/// steady-state step of patches 8 to 12 (the patch change fades rather than
/// cuts)."
/// Assumptions: the sweep is linear from the centre of band 8 to the centre
/// of band 12, written once per 64-frame block while droning at 220 Hz with
/// the other macros at 0.5; steady-state steps are the largest over 1 s of
/// each droning patch 8 to 12.
#[test]
fn harmonics_sweep_between_patches_fades_rather_than_cuts() {
    let steady = (8..=12)
        .map(|p| max_step(&drone(p, &[("timbre", 0.5), ("morph", 0.5)]).render(seconds(1.0))))
        .fold(0.0f32, f32::max);
    let mut v = drone(8, &[("timbre", 0.5), ("morph", 0.5)]);
    let x = v.sweep(
        "harmonics",
        patch_harmonics(8),
        patch_harmonics(12),
        seconds(0.4),
    );
    assert!(peak(&x) > 0.01, "sweep is silent");
    let step = max_step(&x);
    assert!(
        step <= 1.5 * steady,
        "largest step {step} in the sweep exceeds 1.5x steady state ({steady})"
    );
}
