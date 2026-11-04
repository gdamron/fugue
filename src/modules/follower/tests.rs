use super::*;

const SR: u32 = 48_000;

fn run(follower: &mut Follower, inputs: [&[f32]; 3]) -> Vec<f32> {
    let frames = inputs[0].len();
    let mut out = Vec::new();
    for start in (0..frames).step_by(MAX_BLOCK) {
        let n = MAX_BLOCK.min(frames - start);
        for (port, input) in inputs.iter().enumerate() {
            follower.input_block_mut(port)[..n].copy_from_slice(&input[start..start + n]);
        }
        follower.process(n);
        out.extend_from_slice(&follower.output_block(0)[..n]);
    }
    out
}

/// A follower with `attack` 0 whose glide to it has run its course.
fn instant() -> Follower {
    let mut follower = Follower::new(SR);
    follower.set_control("attack", 0.0).unwrap();
    run(&mut follower, [&[0.0; 1024], &[0.0; 1024], &[0.0; 1024]]);
    follower
}

fn largest_step(out: &[f32]) -> f32 {
    out.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_step_up_covers_63_percent_in_the_attack_time() {
    let mut follower = Follower::new(SR);
    let ones = [1.0; 1000];
    let out = run(&mut follower, [&ones, &[0.0; 1000], &[0.0; 1000]]);
    // 480 frames of the default 0.01 s at 48 kHz: 1 - e^-1.
    assert!(
        (out[479] - (1.0 - (-1.0f32).exp())).abs() < 1e-3,
        "{}",
        out[479]
    );
}

#[test]
fn the_louder_side_and_then_the_sidechain_are_followed() {
    let mut follower = instant();
    let out = run(&mut follower, [&[0.25; 64], &[-0.5; 64], &[0.9; 64]]);
    assert_eq!(out[63], 0.5);
    follower.set_control("sidechain_mix", 1.0).unwrap();
    let out = run(&mut follower, [&[0.25; 256], &[-0.5; 256], &[0.9; 256]]);
    assert!(largest_step(&out) < 0.01, "{}", largest_step(&out));
    assert!((out[255] - 0.9).abs() < 1e-6);
}

#[test]
fn the_highpass_starts_clean_and_crossfades_in_and_out() {
    let mut follower = instant();
    follower.set_control("highpass", 500.0).unwrap();
    let dc = [0.8; 48_000];
    let out = run(&mut follower, [&dc, &[0.0; 48_000], &[0.0; 48_000]]);
    // A high-pass takes steady input away; the envelope falls with it.
    assert!(out[47_999] < 1e-3, "{}", out[47_999]);
    follower.set_control("highpass", 0.0).unwrap();
    let out = run(&mut follower, [&dc[..2048], &[0.0; 2048], &[0.0; 2048]]);
    assert!(largest_step(&out) < 0.002, "{}", largest_step(&out));
    assert_eq!(out[2047], 0.8);
}

#[test]
fn a_new_attack_glides_rather_than_jumping_the_envelope() {
    let mut follower = Follower::new(SR);
    follower.set_control("attack", 1.0).unwrap();
    let ones = [1.0; 2048];
    run(&mut follower, [&ones, &[0.0; 2048], &[0.0; 2048]]);
    follower.set_control("attack", 0.0).unwrap();
    let out = run(&mut follower, [&ones, &[0.0; 2048], &[0.0; 2048]]);
    assert!(largest_step(&out) < 0.01, "{}", largest_step(&out));
    assert!((out[2047] - 1.0).abs() < 1e-6);
}

#[test]
fn silence_after_a_peak_reaches_exactly_zero() {
    let mut follower = instant();
    follower.set_control("release", 0.001).unwrap();
    run(&mut follower, [&[1.0; 1024], &[0.0; 1024], &[0.0; 1024]]);
    let quiet = [0.0; 12_000];
    let out = run(&mut follower, [&quiet, &quiet, &quiet]);
    assert_eq!(out[11_999], 0.0);
}
