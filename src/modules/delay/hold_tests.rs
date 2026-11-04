//! Hold: the loop, its fades, and the gap it leaves in the line.

use super::tests::{largest_step, run, sine_437, RATE};
use super::*;

#[test]
fn every_held_pass_after_the_first_is_bit_identical() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("time", 0.05).unwrap();
    delay.heads.settle();
    let noise: Vec<f32> = (0..RATE as usize)
        .map(|n| ((n as u32).wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0 - 0.5)
        .collect();
    run(&mut delay, &noise[..10_000]);
    delay.set_control("hold", 1.0).unwrap();
    let held = run(&mut delay, &noise[10_000..]);
    let length = 2_400;
    let passes: Vec<&[f32]> = held.chunks(length).skip(1).take(10).collect();
    for pass in &passes[1..] {
        assert_eq!(
            pass.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            passes[0].iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }
    assert!(passes[0].iter().any(|v| v.abs() > 0.1));
}

#[test]
fn hold_released_returns_to_the_live_delay() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("time", 0.01).unwrap();
    delay.heads.settle();
    run(&mut delay, &vec![0.25; 4_800]);
    delay.set_control("hold", 1.0).unwrap();
    run(&mut delay, &vec![0.0; 4_800]);
    delay.set_control("hold", 0.0).unwrap();
    let out = run(&mut delay, &vec![0.5; 4_800]);
    assert_eq!(out[out.len() - 1], 0.5);
}

/// Hold leaves a gap in what the line recorded; one delay time after
/// release, the delay's own head passes over it as a fade, not a step.
#[test]
fn the_gap_hold_leaves_in_the_line_is_heard_without_a_step() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("time", 0.5).unwrap();
    delay.heads.settle();
    let tone: Vec<f32> = (0..96_000)
        .map(|n| (n as f32 * 437.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
        .collect();
    let mut out = run(&mut delay, &tone[..30_000]);
    delay.set_control("hold", 1.0).unwrap();
    out.extend(run(&mut delay, &tone[30_000..50_000]));
    delay.set_control("hold", 0.0).unwrap();
    out.extend(run(&mut delay, &tone[50_000..]));
    // A 437 Hz sine at 0.5 moves at most about 0.03 a sample.
    let largest = out[24_001..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max);
    assert!(largest < 0.035, "step of {largest}");
}

/// Hold released and asked for again while its loop still sounds, once
/// (the slice is kept) and over and over (it no longer fits and a fresh one
/// waits for the old to fade): never a step, never a read outside the line.
#[test]
fn hold_toggled_while_its_loop_fades_does_not_click() {
    for toggle_every in [64, 128, 200] {
        let mut delay = Delay::new(RATE, 1.0);
        delay.set_control("time", 0.05).unwrap();
        delay.heads.settle();
        let tone = sine_437(96_000);
        let mut out = run(&mut delay, &tone[..12_000]);
        delay.set_control("hold", 1.0).unwrap();
        out.extend(run(&mut delay, &tone[12_000..24_000]));
        let mut at = 24_000;
        for turn in 0..200 {
            delay.set_control("hold", (turn % 2) as f32).unwrap();
            out.extend(run(&mut delay, &tone[at..at + toggle_every]));
            at += toggle_every;
        }
        delay.set_control("hold", 0.0).unwrap();
        out.extend(run(&mut delay, &tone[at..at + 4_800]));
        let largest = largest_step(&out[2_401..]);
        assert!(
            largest < 0.035,
            "toggling every {toggle_every}: step of {largest}"
        );
    }
}
