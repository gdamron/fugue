use super::*;

const SR: u32 = 48_000;

fn run(gain: &mut Gain, audio: &[f32], modulation: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    for (a, m) in audio.chunks(MAX_BLOCK).zip(modulation.chunks(MAX_BLOCK)) {
        gain.input_block_mut(0)[..a.len()].copy_from_slice(a);
        gain.input_block_mut(1)[..m.len()].copy_from_slice(m);
        gain.process(a.len());
        out.extend_from_slice(&gain.output_block(0)[..a.len()]);
    }
    out
}

fn tone(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| 0.7 * (n as f32 * 0.031).sin())
        .collect()
}

#[test]
fn zero_decibels_passes_bit_for_bit() {
    let input = tone(1000);
    assert_eq!(run(&mut Gain::new(SR), &input, &[0.0; 1000]), input);
}

#[test]
fn modulation_adds_decibels_times_its_depth() {
    let mut gain = Gain::new(SR);
    gain.set_control("decibels", -6.0).unwrap();
    gain.set_control("decibels_mod_depth", 2.0).unwrap();
    run(&mut gain, &[0.0; 512], &[0.0; 512]);
    let out = run(&mut gain, &[0.5; 256], &[3.0; 256]);
    // -6 + 3 × 2 = 0 dB.
    assert!(
        out.iter().all(|&y| (y - 0.5).abs() < 1e-6),
        "{:?}",
        &out[..4]
    );
    let boosted = run(&mut gain, &[0.01; 256], &[100.0; 256]);
    let most = 0.01 * decibels_to_level(MAX_DECIBELS);
    assert!(boosted.iter().all(|&y| (y - most).abs() < 1e-6));
}

#[test]
fn a_decibels_change_glides_for_about_a_block() {
    let mut gain = Gain::new(SR);
    run(&mut gain, &[1.0; 128], &[0.0; 128]);
    gain.set_control("decibels", 12.0).unwrap();
    let out = run(&mut gain, &[1.0; 256], &[0.0; 256]);
    let step = out.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
    assert!(step < 0.05, "largest step {step}");
    let landed = control_glide_frames(SR) as usize - 1;
    assert!(out[landed - 1] < out[landed]);
    assert!(out[landed..]
        .iter()
        .all(|&y| (y - decibels_to_level(12.0)).abs() < 1e-5));
}

#[test]
fn invert_crossfades_to_the_negated_signal() {
    let mut gain = Gain::new(SR);
    gain.set_control("invert", 1.0).unwrap();
    let out = run(&mut gain, &[0.5; 2400], &[0.0; 2400]);
    let step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    // A sign swing of 1.0 over 960 frames at 0.5: steps of 1/960.
    assert!(step < 0.0011, "largest step {step}");
    assert_eq!(out[960..], [-0.5; 1440]);
}

#[test]
fn non_finite_input_and_modulation_are_ignored() {
    let out = run(&mut Gain::new(SR), &[f32::NAN, 0.5], &[0.0, f32::INFINITY]);
    assert_eq!(out, [0.0, 0.5]);
}

#[test]
fn a_change_glides_when_run_a_frame_at_a_time() {
    // A feedback loop runs its modules one frame per call.
    let mut gain = Gain::new(SR);
    gain.set_control("decibels", -20.0).unwrap();
    let out: Vec<f32> = (0..400)
        .map(|_| run(&mut gain, &[1.0], &[0.0])[0])
        .collect();
    let step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(step < 0.05, "largest step {step}");
    assert!((out[399] - 0.1).abs() < 1e-6);
}

#[test]
fn invert_toggled_back_mid_fade_turns_around_smoothly() {
    let mut gain = Gain::new(SR);
    gain.set_control("invert", 1.0).unwrap();
    let mut out = run(&mut gain, &[0.5; 300], &[0.0; 300]);
    gain.set_control("invert", 0.0).unwrap();
    out.extend(run(&mut gain, &[0.5; 1200], &[0.0; 1200]));
    let step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(step < 0.0011, "largest step {step}");
    assert_eq!(out[1499], 0.5);
}
