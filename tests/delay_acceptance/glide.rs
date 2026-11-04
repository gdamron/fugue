//! Criterion 6: sweeping `time` from 50 ms to 500 ms over 1 s with each
//! `glide` mode.

use super::rig::*;
use super::*;

/// Delay time in seconds at `frame`: 0.05 until 0.5 s, then a linear sweep
/// to 0.5 over 1 s.
fn sweep_time(frame: usize) -> f32 {
    let t = (frame as f32 / SR as f32 - 0.5).clamp(0.0, 1.0);
    0.05 + 0.45 * t
}

fn swept(glide: &str, input: &[f32]) -> Vec<f32> {
    let mut d = delay(json!({ "time": 0.05, "glide": glide }));
    d.run(input, 64, |frame, d| d.set("time", sweep_time(frame)))
}

/// Criterion 6, crossfade: no more than 1.5x the dry input's largest step.
/// A step in the read position (a click) would be a jump of up to the sine's
/// full 0.5 amplitude against a dry step of about 0.03.
#[test]
fn crossfade_sweep_steps_no_more_than_1_5_times_the_dry_input() {
    let input = sine(4 * SR as usize, 440.0, 0.5);
    let out = swept("crossfade", &input);
    assert!(peak(&out) > 0.4, "the sine is heard");
    let (wet, dry) = (max_step(&out), max_step(&input));
    assert!(
        wet <= 1.5 * dry,
        "largest step {wet} exceeds 1.5 x dry {dry}"
    );
}

/// Criterion 6, slew: the read position glides like a tape head. A 480 Hz
/// sine has a 100-sample period, so the k-th rising zero crossing of the
/// output is the k-th of the input plus the delay in force: D_k = c_k -
/// 100(k + 1). The sweep moves the delay at 0.45 s per s, so the output runs
/// at 0.55x pitch and D grows by 100 x 0.45 / 0.55 = 82 samples per output
/// period once the glide has caught up. D must follow smoothly: its second
/// difference (a pitch step or a jump of the read position shows up as up to
/// 82, a crossfade as spurious crossings) stays under 15, which a smoother
/// with a time constant anywhere near 0.1 s meets with margin.
/// Assumption: the sweep is applied as control writes every 64 samples.
#[test]
fn slew_sweep_glides_the_read_position_without_a_step() {
    let input = sine(4 * SR as usize, 480.0, 0.5);
    let out = swept("slew", &input);
    let crossings = rising_crossings(&out, 0.0);
    let delays: Vec<f64> = crossings
        .iter()
        .enumerate()
        .map(|(k, c)| c - 100.0 * (k + 1) as f64)
        .collect();
    assert!(
        delays.len() > 800,
        "a steady sine comes out ({} crossings)",
        delays.len()
    );
    assert!(
        (delays[0] - 2_400.0).abs() < 3.0,
        "starts at 50 ms: {}",
        delays[0]
    );
    let last = *delays.last().unwrap();
    assert!((last - 24_000.0).abs() < 3.0, "ends at 500 ms: {last}");
    let first_diff: Vec<f64> = delays.windows(2).map(|w| w[1] - w[0]).collect();
    let bend = first_diff.iter().cloned().fold(0.0, f64::max);
    assert!(
        (60.0..95.0).contains(&bend),
        "pitch bends to 0.55x (delay grows {bend} per period)"
    );
    let second = first_diff
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f64::max);
    assert!(
        second < 15.0,
        "read position steps: second difference {second} samples"
    );
}
