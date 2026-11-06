//! Pan glides: a pan change moves the pan, not the gains, over
//! [`GAIN_RAMP_MS`], and the pan CV input stays sample-exact.

use super::tests::{left_blocks, ramp_frames, BLOCK};
use super::*;
use crate::factory::ModuleFactory;

/// How close a frame is to the pan law at the pan it should be at: f32
/// rounding gathered over a glide, far below the step a click would be.
const LAW_TOLERANCE: f32 = 1e-5;
const HARD_LEFT: f32 = -1.0;
const HARD_RIGHT: f32 = 1.0;

/// The left and right gains of the equal-power pan law at `pan`, with the
/// far side of a hard pan exactly silent.
fn law(pan: f32) -> (f32, f32) {
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (
        if pan >= 1.0 { 0.0 } else { angle.cos() },
        if pan <= -1.0 { 0.0 } else { angle.sin() },
    )
}

/// Plays `frames` single frames of a constant 1.0 on channel 0 and returns
/// each frame's left and right output.
fn stereo_outputs(mixer: &mut Mixer, frames: u32) -> Vec<(f32, f32)> {
    (0..frames)
        .map(|_| {
            mixer.set_input("audio.0", 1.0).unwrap();
            mixer.process(1);
            (
                mixer.get_output("audio_left").unwrap(),
                mixer.get_output("audio_right").unwrap(),
            )
        })
        .collect()
}

/// Whether each output is the pan law at a pan moving from `from` to `to` in
/// equal steps, landing on it on the last frame.
fn pans_glide(outputs: &[(f32, f32)], from: f32, to: f32) -> bool {
    let frames = ramp_frames() as f32;
    outputs.iter().enumerate().all(|(k, &(left, right))| {
        let pan = from + (to - from) * (k + 1) as f32 / frames;
        let (l, r) = law(pan);
        (left - l).abs() <= LAW_TOLERANCE && (right - r).abs() <= LAW_TOLERANCE
    })
}

#[test]
fn the_first_block_starts_at_the_controls_pan() {
    let mut mixer = Mixer::new(1).with_pan(0, HARD_LEFT);
    assert_eq!(stereo_outputs(&mut mixer, 1)[0], law(HARD_LEFT));
}

#[test]
fn a_pan_change_glides_the_pan_through_the_pan_law() {
    let mut mixer = Mixer::new(1);
    stereo_outputs(&mut mixer, 1);
    mixer.set_pan(0, HARD_RIGHT);
    let outputs = stereo_outputs(&mut mixer, ramp_frames());
    assert!(pans_glide(&outputs, 0.0, HARD_RIGHT), "{outputs:?}");
    assert_eq!(*outputs.last().unwrap(), law(HARD_RIGHT));
    assert_eq!(mixer.get_control("pan.0").unwrap(), HARD_RIGHT);
}

#[test]
fn a_pan_glide_keeps_equal_power() {
    let mut mixer = Mixer::new(1).with_pan(0, HARD_LEFT);
    stereo_outputs(&mut mixer, 1);
    mixer.set_pan(0, HARD_RIGHT);
    for (left, right) in stereo_outputs(&mut mixer, ramp_frames()) {
        let power = left * left + right * right;
        assert!((power - 1.0).abs() <= LAW_TOLERANCE, "{power}");
    }
}

#[test]
fn a_pan_change_mid_glide_turns_back_without_a_jump() {
    let mut mixer = Mixer::new(1).with_pan(0, HARD_LEFT);
    stereo_outputs(&mut mixer, 1);
    mixer.set_pan(0, HARD_RIGHT);
    let half = ramp_frames() / 2;
    let there = stereo_outputs(&mut mixer, half);
    let turned_at = HARD_LEFT + (HARD_RIGHT - HARD_LEFT) * half as f32 / ramp_frames() as f32;
    assert!(pans_glide(&there, HARD_LEFT, HARD_RIGHT), "{there:?}");
    mixer.set_pan(0, HARD_LEFT);
    let back = stereo_outputs(&mut mixer, ramp_frames());
    assert!(pans_glide(&back, turned_at, HARD_LEFT), "{back:?}");
}

#[test]
fn the_pan_cv_input_moves_on_the_frame_it_changes() {
    let mut mixer = Mixer::new(1);
    mixer.set_input("pan.0", 0.0).unwrap();
    stereo_outputs(&mut mixer, 1);
    mixer.set_input("pan.0", HARD_RIGHT).unwrap();
    let outputs = stereo_outputs(&mut mixer, 1);
    assert_eq!(outputs[0], law(HARD_RIGHT));
}

#[test]
fn the_pan_cv_input_adds_to_a_gliding_pan() {
    const CV: f32 = -0.5;
    let mut mixer = Mixer::new(1);
    mixer.set_input("pan.0", CV).unwrap();
    stereo_outputs(&mut mixer, 1);
    mixer.set_pan(0, HARD_RIGHT);
    let outputs = stereo_outputs(&mut mixer, ramp_frames());
    assert!(pans_glide(&outputs, CV, HARD_RIGHT + CV), "{outputs:?}");
}

#[test]
fn the_factory_times_the_pan_glide_from_the_sample_rate() {
    const RATE: u32 = 44_100;
    let config = serde_json::json!({ "channel_count": 1 });
    let mut built = MixerFactory.build(RATE, &config).unwrap();
    let mixer = built.module.module_mut();
    left_blocks(mixer, 1);
    mixer.set_control("pan.0", HARD_RIGHT).unwrap();
    let frames = gain_ramp_frames(RATE) as usize;
    let outputs = left_blocks(mixer, frames.div_ceil(BLOCK) + 1);
    let (hard_right_left, _) = law(HARD_RIGHT);
    assert!(outputs[frames - 2] > hard_right_left, "{outputs:?}");
    assert!(outputs[frames - 1..].iter().all(|&v| v == hard_right_left));
}

/// One frame of `input` on channel 0 at `pan`, as (left, right).
fn one_frame(pan: f32, input: f32) -> (f32, f32) {
    let mut mixer = Mixer::new(1).with_pan(0, pan);
    mixer.set_input("audio.0", input).unwrap();
    mixer.process(1);
    (
        mixer.get_output("audio_left").unwrap(),
        mixer.get_output("audio_right").unwrap(),
    )
}

#[test]
fn a_hard_pan_leaves_exact_silence_on_the_far_side() {
    for input in [1.0f32, -1.0, 0.5, 1e-3, 2.0] {
        let (_, right) = one_frame(HARD_LEFT, input);
        let (left, _) = one_frame(HARD_RIGHT, input);
        assert_eq!(left, 0.0, "hard right, input {input}");
        assert_eq!(right, 0.0, "hard left, input {input}");
    }
}

#[test]
fn a_pan_cv_past_the_end_is_clamped_to_exact_silence() {
    let mut mixer = Mixer::new(1).with_pan(0, 0.5);
    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("pan.0", 0.75).unwrap();
    stereo_outputs(&mut mixer, 1);
    assert_eq!(stereo_outputs(&mut mixer, 1)[0].0, 0.0);
}

#[test]
fn the_pan_law_inside_the_ends_is_unchanged() {
    let just_inside = 1.0 - f32::EPSILON;
    for pan in [-0.999, -0.5, 0.0, 0.3, 0.999, just_inside, -just_inside] {
        let (left, right) = one_frame(pan, 1.0);
        let (want_left, want_right) = law(pan);
        assert!((left - want_left).abs() <= 1e-6, "pan {pan}: {left}");
        assert!((right - want_right).abs() <= 1e-6, "pan {pan}: {right}");
    }
}
