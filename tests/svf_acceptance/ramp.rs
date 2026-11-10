use fugue::dsp::BlockRamp;

/// Tolerance for f32 arithmetic on values of this magnitude.
fn tol(start: f32, target: f32) -> f64 {
    2e-6 * start.abs().max(target.abs()).max(1.0) as f64
}

/// Runs one block with `steps`, returning the per-frame values.
fn block(ramp: &mut BlockRamp, target: f32, frames: usize) -> Vec<f32> {
    ramp.steps(target, frames).collect()
}

/// Checks everything the spec says about one block that starts at `start`.
fn check_block(start: f32, target: f32, got: &[f32]) {
    let n = got.len();
    let step = target as f64 - start as f64;
    let t = tol(start, target);

    // The last value is the target, exactly.
    assert_eq!(got[n - 1], target, "{start} -> {target} over {n}");

    // No single-sample jump (counting the one from the previous block's last
    // value) larger than step / block_size.
    let mut prev = start;
    for (i, &v) in got.iter().enumerate() {
        let jump = (v as f64 - prev as f64).abs();
        assert!(
            jump <= step.abs() / n as f64 + t,
            "{start} -> {target} over {n}: jump {jump} at {i}"
        );
        prev = v;
    }

    // Linear: equal increments, each step / n.
    let mut prev = start;
    for (i, &v) in got.iter().enumerate() {
        let inc = v as f64 - prev as f64;
        assert!(
            (inc - step / n as f64).abs() <= t,
            "{start} -> {target} over {n}: increment {inc} at {i}, want {}",
            step / n as f64
        );
        prev = v;
    }

    // And therefore on the line from start to target.
    for (i, &v) in got.iter().enumerate() {
        let want = start as f64 + step * (i + 1) as f64 / n as f64;
        assert!(
            (v as f64 - want).abs() <= t,
            "{start} -> {target} over {n}: frame {i}"
        );
    }
}

#[test]
fn snap_jumps_at_once_and_the_next_block_ramps_from_there() {
    let mut ramp = BlockRamp::new(0.0);
    ramp.snap(0.8);
    assert_eq!(ramp.value(), 0.8);
    for frames in [1, 7, 64, 333] {
        let mut r = BlockRamp::new(0.0);
        r.snap(0.8);
        check_block(0.8, 0.2, &block(&mut r, 0.2, frames));
        assert_eq!(r.value(), 0.2);
    }
    // Snapping mid-ramp discards the old position.
    let mut ramp = BlockRamp::new(0.0);
    block(&mut ramp, 1.0, 64);
    ramp.snap(-1.0);
    assert_eq!(ramp.value(), -1.0);
    check_block(-1.0, 0.0, &block(&mut ramp, 0.0, 64));
}
