//! Cutoff and resonance writes glide in; inputs and starting values do not.

use std::f32::consts::TAU;

use super::glide::{ControlGlide, GLIDE_MS};
use super::*;

const RATE: u32 = 48_000;
const AMPLITUDE: f32 = 0.5;
const TONE_HZ: f32 = 1000.0;

fn sine(n: usize) -> f32 {
    AMPLITUDE * (TAU * TONE_HZ * n as f32 / RATE as f32).sin()
}

/// A sample where the sine peaks: a jump here asks the most of the filter.
const PEAK: usize = 4_812;

fn glide_frames() -> usize {
    (RATE as f32 * GLIDE_MS / 1000.0).round() as usize
}

/// Runs `filter` over the sine from sample `from`, one sample at a time.
fn run(filter: &mut Filter, from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames)
        .map(|n| {
            filter.set_input("audio", sine(n)).unwrap();
            filter.process(1);
            filter.get_output("audio").unwrap()
        })
        .collect()
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn opening_a_dark_lowpass_at_once_does_not_click() {
    let dry: Vec<f32> = (0..RATE as usize).map(sine).collect();
    let dry_step = largest_step(&dry);

    let mut filter = Filter::new(RATE).with_cutoff(20.0);
    let before = run(&mut filter, 0, PEAK);
    filter.set_control("cutoff", 20_000.0).unwrap();
    let after = run(&mut filter, PEAK, 4_800);

    let mut heard = before;
    heard.extend(after);
    let step = largest_step(&heard);
    assert!(
        step <= 1.5 * dry_step,
        "largest step {step} against the dry signal's {dry_step}"
    );
}

#[test]
fn closing_an_open_lowpass_and_moving_resonance_do_not_click() {
    let dry: Vec<f32> = (0..RATE as usize).map(sine).collect();
    let dry_step = largest_step(&dry);

    let mut filter = Filter::new(RATE).with_cutoff(20_000.0);
    let mut heard = run(&mut filter, 0, PEAK);
    filter.set_control("cutoff", 20.0).unwrap();
    filter.set_control("resonance", 0.9).unwrap();
    heard.extend(run(&mut filter, PEAK, 4_800));
    filter.set_control("cutoff", 1_000.0).unwrap();
    filter.set_control("resonance", 0.0).unwrap();
    heard.extend(run(&mut filter, PEAK + 4_800, 4_800));
    let step = largest_step(&heard);
    assert!(
        step <= 1.5 * dry_step,
        "largest step {step} against the dry signal's {dry_step}"
    );
}

#[test]
fn a_write_before_the_filter_is_heard_lands_at_once() {
    let mut moved = Filter::new(RATE).with_cutoff(20.0);
    moved.set_control("cutoff", 5_000.0).unwrap();
    moved.set_control("resonance", 0.5).unwrap();
    let mut built = Filter::new(RATE).with_cutoff(5_000.0).with_resonance(0.5);
    assert_eq!(run(&mut moved, 0, 960), run(&mut built, 0, 960));
}

#[test]
fn processing_no_frames_does_not_count_as_heard() {
    let mut moved = Filter::new(RATE).with_cutoff(20.0);
    moved.process(0);
    moved.set_control("cutoff", 5_000.0).unwrap();
    let mut built = Filter::new(RATE).with_cutoff(5_000.0);
    assert_eq!(run(&mut moved, 0, 960), run(&mut built, 0, 960));
}

#[test]
fn a_glide_lands_exactly_on_the_written_value() {
    let mut glide = ControlGlide::new(RATE, 20.0, 0.0);
    glide.heard();
    glide.cutoff_to(20_000.0);
    glide.resonance_to(0.5);
    let frames = glide_frames();
    let mut cutoffs = Vec::new();
    let mut resonances = Vec::new();
    for _ in 0..frames + 2 {
        cutoffs.push(glide.next_cutoff(20_000.0));
        resonances.push(glide.next_resonance(0.5));
    }
    // Octaves: halfway through, the cutoff is the two ends' geometric mean.
    let middle = cutoffs[frames / 2 - 1];
    assert!((middle - 632.5).abs() < 5.0, "halfway at {middle} Hz");
    assert!(cutoffs.windows(2).all(|pair| pair[1] >= pair[0]));
    assert_eq!(cutoffs[frames - 1], 20_000.0);
    assert_eq!(resonances[frames - 1], 0.5);
    assert_eq!(cutoffs[frames + 1], 20_000.0);
    assert_eq!(resonances[frames + 1], 0.5);
}

#[test]
fn a_resting_glide_returns_the_control_value_exactly() {
    let mut glide = ControlGlide::new(RATE, 1234.5, 0.25);
    glide.heard();
    assert_eq!(glide.next_cutoff(1234.5), 1234.5);
    assert_eq!(glide.next_resonance(0.25), 0.25);
}

#[test]
fn the_cutoff_input_still_moves_the_filter_at_once() {
    // An input-driven jump is not smoothed: the same jump through the input
    // steps the output far more than the glided control write does.
    let cutoff = inputs::INPUTS.iter().position(|&port| port == "cutoff");
    let cutoff = cutoff.expect("a cutoff input");
    let mut filter = Filter::new(RATE);
    filter.set_input_connected(cutoff, true);
    let mut heard = Vec::new();
    for n in 0..9_600 {
        let hz = if n < PEAK { 20.0 } else { 20_000.0 };
        filter.set_input("cutoff", hz).unwrap();
        filter.set_input("audio", sine(n)).unwrap();
        filter.process(1);
        heard.push(filter.get_output("audio").unwrap());
    }
    let dry: Vec<f32> = (0..RATE as usize).map(sine).collect();
    assert!(largest_step(&heard) > 2.0 * largest_step(&dry));
}
