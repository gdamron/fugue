use super::*;

#[test]
fn the_curve_is_flat_below_bends_through_the_knee_and_slopes_above() {
    let s = 1.0 / 4.0 - 1.0;
    assert_eq!(gain_change(-40.0, -20.0, s, 6.0), 0.0);
    // At the threshold the knee is halfway: (W/2)² / 2W · s = W/8 · s.
    assert!((gain_change(-20.0, -20.0, s, 6.0) - 0.75 * s).abs() < 1e-6);
    assert!((gain_change(0.0, -20.0, s, 6.0) + 15.0).abs() < 1e-5);
    assert!((gain_change(0.0, -20.0, s, 0.0) + 15.0).abs() < 1e-5);
    // The knee meets both lines at its edges.
    assert!(gain_change(-23.0, -20.0, s, 6.0).abs() < 1e-6);
    assert!((gain_change(-17.0, -20.0, s, 6.0) - 3.0 * s).abs() < 1e-5);
}

#[test]
fn a_ratio_of_one_leaves_every_level_alone() {
    for x in [-120.0, -20.0, 0.0] {
        assert_eq!(gain_change(x, -20.0, 0.0, 6.0), 0.0);
    }
}

#[test]
fn the_module_reads_a_level_and_glides_a_threshold_change() {
    let mut curve = GainCurve::new(48_000);
    curve.set_control("knee", 0.0).unwrap();
    let out = run(&mut curve, 1024);
    assert!((out[1023] + 15.0).abs() < 1e-4);
    curve.set_control("threshold", 0.0).unwrap();
    let out = run(&mut curve, 256);
    assert!(
        largest_step(&out) < 0.2,
        "largest step {}",
        largest_step(&out)
    );
    assert!(out[255].abs() < 1e-4);
}

#[test]
fn ratio_and_knee_changes_glide_when_run_a_frame_at_a_time() {
    let mut curve = GainCurve::new(48_000);
    run(&mut curve, 1024);
    curve.set_control("ratio", 1.0).unwrap();
    curve.set_control("knee", 0.0).unwrap();
    let out: Vec<f32> = (0..256).map(|_| run(&mut curve, 1)[0]).collect();
    assert!(out.iter().all(|y| y.is_finite()));
    assert!(
        largest_step(&out) < 0.2,
        "largest step {}",
        largest_step(&out)
    );
    assert_eq!(out[255], 0.0);
}

/// Runs `frames` frames of a steady 0 dBFS level.
fn run(curve: &mut GainCurve, frames: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for start in (0..frames).step_by(MAX_BLOCK) {
        let n = MAX_BLOCK.min(frames - start);
        curve.input_block_mut(0)[..n].fill(-1.0);
        curve.process(n);
        out.extend_from_slice(&curve.output_block(0)[..n]);
    }
    out
}

fn largest_step(out: &[f32]) -> f32 {
    out.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}
