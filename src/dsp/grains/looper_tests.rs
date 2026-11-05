use super::*;

const SR: u32 = 48_000;

fn tone(n: usize) -> f32 {
    (std::f32::consts::TAU * 220.0 * n as f32 / SR as f32).sin()
}

fn largest_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Runs `frames` of `input(n)` through, from frame `from`, writing first.
fn run(
    looper: &mut Looper,
    buffer: &mut Recording,
    shape: &LoopShape,
    recording: bool,
    from: usize,
    frames: usize,
    input: impl Fn(usize) -> f32,
) -> Vec<f32> {
    (from..from + frames)
        .map(|n| {
            buffer.write(input(n), input(n), recording);
            looper.tick(buffer, shape, recording).0
        })
        .collect()
}

#[test]
fn recording_it_delays_the_input_by_position() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let shape = LoopShape {
        position: 0.25,
        ..LoopShape::default()
    };
    let click = |n: usize| if n == 20_000 { 1.0 } else { 0.0 };
    let out = run(&mut looper, &mut buffer, &shape, true, 0, 40_000, click);
    let heard = out.iter().position(|x| x.abs() > 0.5).unwrap();
    let longest = SR as f64 - 2.0 * MARGIN - looper.fade_frames as f64;
    assert_eq!(heard - 20_000, (0.25 * longest).round() as usize);
}

#[test]
fn two_taps_set_the_delay_to_the_time_between() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let shape = LoopShape::default();
    run(&mut looper, &mut buffer, &shape, true, 0, 1_000, |_| 0.0);
    looper.tap();
    run(&mut looper, &mut buffer, &shape, true, 1_000, 6_000, |_| {
        0.0
    });
    looper.tap();
    let click = |n: usize| if n == 20_000 { 1.0 } else { 0.0 };
    let out = run(&mut looper, &mut buffer, &shape, true, 7_000, 30_000, click);
    let heard = 7_000 + out.iter().position(|x| x.abs() > 0.5).unwrap();
    assert_eq!(heard - 20_000, 6_000);
}

#[test]
fn freezing_loops_what_was_heard_without_a_click() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let length = buffer.len() as f64;
    // A loop a little under 0.1 s, so the tone's phase jumps at each seam.
    let shape = LoopShape {
        position: 0.3,
        size: 0.42,
        period: None,
        latency: 0,
    };
    let steady = run(&mut looper, &mut buffer, &shape, true, 0, SR as usize, tone);
    let steady = largest_step(&steady[SR as usize / 2..]);
    // Frozen, nothing new is recorded: silence in, the loop out.
    let frozen = run(
        &mut looper,
        &mut buffer,
        &shape,
        false,
        SR as usize,
        SR as usize,
        |_| 0.0,
    );
    let thawed = run(
        &mut looper,
        &mut buffer,
        &shape,
        true,
        2 * SR as usize,
        SR as usize / 2,
        tone,
    );
    for (label, part) in [("frozen", &frozen), ("thawed", &thawed)] {
        let step = largest_step(part);
        assert!(step <= 1.5 * steady, "{label}: {step} vs {steady}");
    }
    let late = frozen[SR as usize / 2..]
        .iter()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(late > 0.5, "the loop plays on, at {late}");
    // Frozen, the loop and its fades stay behind the still head.
    let behind = (buffer.head() as f64 - looper.at).rem_euclid(length);
    assert!(behind >= MARGIN, "{behind}");
}

#[test]
fn a_synced_period_sets_the_frozen_loop() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let shape = LoopShape {
        position: 0.9,
        size: 0.0,
        period: Some(12_000),
        latency: 0,
    };
    run(&mut looper, &mut buffer, &shape, true, 0, SR as usize, tone);
    run(
        &mut looper,
        &mut buffer,
        &shape,
        false,
        SR as usize,
        2_000,
        tone,
    );
    assert_eq!(
        looper.cycle.map(|left| left.round() as i64 + 2_000),
        Some(12_000)
    );
}

/// Frozen output after `freeze_at`, fed `input` throughout.
fn frozen_loop(size: f32, freeze_at: usize, input: fn(usize) -> f32) -> Vec<f32> {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let mut shape = LoopShape {
        position: 0.1,
        size,
        ..LoopShape::default()
    };
    run(
        &mut looper,
        &mut buffer,
        &shape,
        true,
        0,
        freeze_at - 200,
        tone,
    );
    // Move the delay just before the freeze, so it lands mid-jump.
    shape.position = 0.05;
    run(
        &mut looper,
        &mut buffer,
        &shape,
        true,
        freeze_at - 200,
        200,
        tone,
    );
    run(
        &mut looper,
        &mut buffer,
        &shape,
        false,
        freeze_at,
        SR as usize,
        input,
    )
}

#[test]
fn the_frozen_loop_never_reads_what_the_write_fade_overwrites() {
    // The head runs on for its fade after a freeze, still writing: the
    // loop must not reach it, from the first cycle or any after.
    for size in [0.0, 0.3, 1.0] {
        let quiet = frozen_loop(size, 30_000, |_| 0.0);
        let loud = frozen_loop(size, 30_000, |n| tone(n * 3));
        assert_eq!(quiet, loud, "size {size}");
        assert!(quiet.iter().any(|x| x.abs() > 0.5), "size {size} plays");
    }
}

#[test]
fn a_stray_tap_long_after_the_last_starts_afresh() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut looper = Looper::new(SR);
    let shape = LoopShape::default();
    looper.tap();
    run(
        &mut looper,
        &mut buffer,
        &shape,
        true,
        0,
        2 * SR as usize,
        |_| 0.0,
    );
    looper.tap();
    assert_eq!(looper.tapped, None);
}

/// Frames behind the write head the reader last read at.
fn behind(looper: &Looper, buffer: &Recording) -> f64 {
    (buffer.head() as f64 - looper.at).rem_euclid(buffer.len() as f64)
}

/// A recording looper with `glide` seconds set, a second of tone in.
fn gliding(glide: f32, position: f32) -> (Looper, Recording, LoopShape, f32) {
    let mut buffer = Recording::new(SR, 4 * SR as usize);
    let mut looper = Looper::new(SR);
    looper.set_glide(glide);
    let shape = LoopShape {
        position,
        ..LoopShape::default()
    };
    let steady = run(&mut looper, &mut buffer, &shape, true, 0, SR as usize, tone);
    let steady = largest_step(&steady[SR as usize / 2..]);
    (looper, buffer, shape, steady)
}

#[test]
fn a_slow_sweep_glides_smoothly_and_bends_the_pitch() {
    let (mut looper, mut buffer, mut shape, steady) = gliding(0.5, 0.1);
    let mut out = Vec::new();
    let mut slowest = f64::MAX;
    // 0.1 to 0.3 over four seconds, a 512-frame block at a time.
    for block in 0..375 {
        shape.position = 0.1 + 0.2 * block as f32 / 375.0;
        let from = SR as usize + block * 512;
        let before = behind(&looper, &buffer);
        out.extend(run(&mut looper, &mut buffer, &shape, true, from, 512, tone));
        let grew = behind(&looper, &buffer) - before;
        slowest = slowest.min(1.0 - grew / 512.0);
        assert_eq!(looper.fade, 0, "block {block} glides, never jumps");
    }
    let step = largest_step(&out);
    assert!(step <= 1.5 * steady, "{step} vs {steady}");
    // The delay grows 0.2 of ~4 s over 4 s: the reader runs ~20% slow.
    assert!(slowest < 0.85 && slowest > 0.5, "read speed {slowest}");
}

#[test]
fn a_leap_still_crossfades_and_lands_on_the_new_delay() {
    let (mut looper, mut buffer, mut shape, steady) = gliding(0.5, 0.1);
    shape.position = 0.9;
    let from = SR as usize;
    let first = run(&mut looper, &mut buffer, &shape, true, from, 1, tone);
    assert!(looper.fade > 0, "a leap crossfades");
    let out = run(
        &mut looper,
        &mut buffer,
        &shape,
        true,
        from + 1,
        2_000,
        tone,
    );
    let longest = buffer.len() as f64 - 2.0 * MARGIN - looper.fade_frames as f64;
    let due = (0.9 * longest).round();
    assert!((behind(&looper, &buffer) - due).abs() < 1e-6);
    let step = largest_step(&[first, out].concat());
    assert!(step <= 1.5 * steady, "{step} vs {steady}");
}

#[test]
fn a_gliding_reader_keeps_clear_of_the_write_head() {
    for glide in [0.001, 0.05, 2.0] {
        let (mut looper, mut buffer, mut shape, _) = gliding(glide, 0.0);
        let length = buffer.len() as f64;
        // Back and forth across the whole range, within each glide's reach.
        for step in 0..2_000 {
            let phase = (step % 400) as f32 / 400.0;
            shape.position =
                0.24 * (1.0 - (2.0 * phase - 1.0).abs()) + if step >= 1_000 { 0.75 } else { 0.0 };
            for _ in 0..48 {
                buffer.write(0.0, 0.0, true);
                looper.tick(&buffer, &shape, true);
                let behind = behind(&looper, &buffer);
                assert!(
                    behind >= MARGIN && behind <= length - MARGIN,
                    "{glide}: {behind}"
                );
            }
        }
    }
}

#[test]
fn turning_glide_on_and_off_mid_move_does_not_click() {
    let (mut looper, mut buffer, mut shape, steady) = gliding(0.0, 0.1);
    let mut out = Vec::new();
    for (block, glide) in [(0, 0.3), (40, 0.0), (80, 1.0), (120, 0.0)] {
        looper.set_glide(glide);
        for i in 0..40 {
            shape.position = 0.1 + 0.002 * (block + i) as f32;
            let from = SR as usize + (block + i) * 512;
            out.extend(run(&mut looper, &mut buffer, &shape, true, from, 512, tone));
        }
    }
    let step = largest_step(&out);
    assert!(step <= 1.5 * steady, "{step} vs {steady}");
}
