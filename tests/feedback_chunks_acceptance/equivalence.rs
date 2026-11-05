//! Criterion "output is bit-identical at block sizes 16, 64 and 256, and
//! within 1e-6 of a fused native reference".

use super::*;

const FRAMES: usize = 48_000;
/// Delay times in samples (0.1 s and 0.075 s) and the vca gains of the loop.
const DA: usize = 4800;
const DB: usize = 3600;
const GAIN: f32 = 0.7;
const LEVEL: f64 = 0.2;

/// Two delays in a cross-feedback loop through vcas, fed with noise.
/// Left is delay A, right delay B.
fn noise_ping_pong(min_time: Option<f64>) -> Doc {
    let cfg = |time| delay_cfg(time, min_time);
    Doc::new()
        .add("n", "noise", json!({ "seed": 7, "level": LEVEL }))
        .add("a", "delay", cfg(0.1))
        .add("b", "delay", cfg(0.075))
        .add("ab", "vca", json!({ "level": GAIN }))
        .add("ba", "vca", json!({ "level": GAIN }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("n", "white", "a", "audio")
        .wire("a", "audio", "ab", "audio")
        .wire("ab", "audio", "b", "audio")
        .wire("b", "audio", "ba", "audio")
        .wire("ba", "audio", "a", "audio")
        .wire("a", "audio", "dac", "audio_left")
        .wire("b", "audio", "dac", "audio_right")
}

/// The noise alone, as the dry input of the reference.
fn dry() -> Vec<f32> {
    let doc = Doc::new()
        .add("n", "noise", json!({ "seed": 7, "level": LEVEL }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("n", "white", "dac", "audio_left");
    offline(&doc, FRAMES, 64).0
}

/// The same loop as one plain Rust loop: whole-sample delay lines and gains,
/// every pass exactly the delays long, levels inside the soft clip's linear
/// range so the line records what it is given.
fn native(dry: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = dry.len();
    let (mut a, mut b) = (vec![0.0f32; n], vec![0.0f32; n]);
    let (mut in_a, mut in_b) = (vec![0.0f32; n], vec![0.0f32; n]);
    for i in 0..n {
        a[i] = if i >= DA { in_a[i - DA] } else { 0.0 };
        b[i] = if i >= DB { in_b[i - DB] } else { 0.0 };
        in_b[i] = a[i] * GAIN;
        in_a[i] = dry[i] + b[i] * GAIN;
    }
    (a, b)
}

fn check(min_time: Option<f64>) {
    let dry = dry();
    assert!(dry.iter().any(|v| v.abs() > 0.05), "the noise is audible");
    let (want_a, want_b) = native(&dry);
    let peak = want_a
        .iter()
        .chain(&want_b)
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak < 0.75,
        "the reference stays in the soft clip's linear range ({peak})"
    );

    let doc = noise_ping_pong(min_time);
    let (left, right) = offline(&doc, FRAMES, 64);
    for block in [16, 256] {
        let (l, r) = offline(&doc, FRAMES, block);
        assert_bits(
            &l,
            &left,
            &format!("min_time {min_time:?}: left, block {block} vs 64"),
        );
        assert_bits(
            &r,
            &right,
            &format!("min_time {min_time:?}: right, block {block} vs 64"),
        );
    }
    assert_close(
        &left,
        &want_a,
        1e-6,
        &format!("min_time {min_time:?}: delay A vs native"),
    );
    assert_close(
        &right,
        &want_b,
        1e-6,
        &format!("min_time {min_time:?}: delay B vs native"),
    );
}

/// Chunks of one frame (the default `min_time`).
#[test]
fn cross_feedback_matches_the_native_loop_at_every_block_size() {
    check(None);
}

/// Chunks of 2400 frames, longer than any block: each chunk is cut short by
/// the block's end, yet the output does not depend on where blocks fall.
#[test]
fn cross_feedback_with_long_chunks_matches_the_native_loop_at_every_block_size() {
    check(Some(0.05));
}
