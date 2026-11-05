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
