use fugue::dsp::BlockRamp;

const SIZES: [usize; 6] = [1, 2, 7, 64, 333, 1024];

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
fn a_step_ramps_linearly_across_the_block() {
    // Both directions, positive and negative values, small and large magnitudes.
    for (start, target) in [
        (0.0, 1.0),
        (1.0, 0.0),
        (0.25, 0.75),
        (0.75, 0.25),
        (-0.5, 0.5),
        (0.5, -0.5),
        (220.0, 880.0),
        (880.0, 220.0),
    ] {
        for frames in SIZES {
            let mut ramp = BlockRamp::new(start);
            let got = block(&mut ramp, target, frames);
            assert_eq!(got.len(), frames);
            check_block(start, target, &got);
        }
    }
}

#[test]
fn the_block_ends_on_the_target_and_the_next_starts_from_it() {
    let mut ramp = BlockRamp::new(0.0);
    assert_eq!(ramp.value(), 0.0);
    // Block sizes and step directions change from block to block.
    let plan = [
        (1.0, 64),
        (0.2, 7),
        (0.9, 333),
        (-0.4, 1),
        (-0.4, 64),
        (0.0, 7),
        (0.5, 64),
        (0.5, 333),
    ];
    let mut start = 0.0f32;
    for (target, frames) in plan {
        let got = block(&mut ramp, target, frames);
        check_block(start, target, &got);
        assert_eq!(ramp.value(), target, "value() after the block");
        start = ramp.value();
    }
}

#[test]
fn a_constant_target_holds_steady() {
    let mut ramp = BlockRamp::new(0.3);
    for frames in SIZES {
        assert!(block(&mut ramp, 0.3, frames).iter().all(|&v| v == 0.3));
        assert_eq!(ramp.value(), 0.3);
    }
}

#[test]
fn a_block_of_one_frame_lands_on_the_target() {
    let mut ramp = BlockRamp::new(2.0);
    assert_eq!(block(&mut ramp, -3.0, 1), [-3.0]);
    assert_eq!(ramp.value(), -3.0);
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

#[test]
fn the_steps_iterator_reports_its_exact_length() {
    for frames in SIZES {
        let mut ramp = BlockRamp::new(0.0);
        let mut steps = ramp.steps(1.0, frames);
        assert_eq!(steps.len(), frames);
        assert_eq!(steps.size_hint(), (frames, Some(frames)));
        steps.next();
        assert_eq!(steps.len(), frames - 1);
        assert_eq!(steps.count(), frames - 1);
    }
}

#[test]
fn fill_writes_the_same_ramp_as_steps() {
    for (start, target) in [(0.0, 1.0), (1.0, 0.25), (-0.5, 0.5)] {
        for frames in SIZES {
            let mut by_steps = BlockRamp::new(start);
            let want = block(&mut by_steps, target, frames);

            let mut by_fill = BlockRamp::new(start);
            let mut out = vec![f32::NAN; frames];
            by_fill.fill(target, &mut out);

            check_block(start, target, &out);
            for (i, (a, b)) in out.iter().zip(&want).enumerate() {
                assert!(
                    (*a as f64 - *b as f64).abs() <= tol(start, target),
                    "{frames} frames, frame {i}: fill {a}, steps {b}"
                );
            }
            assert_eq!(by_fill.value(), target);

            // The next block starts from where fill ended.
            let mut next = vec![0.0; frames];
            by_fill.fill(start, &mut next);
            check_block(target, start, &next);
        }
    }
}
