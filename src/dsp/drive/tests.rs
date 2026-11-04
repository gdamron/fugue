use std::f32::consts::TAU;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;

const RATE: u32 = 48_000;
const GLIDE: u32 = 480;

fn sine(hz: f32, peak: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| peak * (TAU * hz * n as f32 / RATE as f32).sin())
        .collect()
}

fn render(drive: &mut DriveProcessor, input: &[f32], block: usize) -> Vec<f32> {
    let mut out = vec![0.0; input.len()];
    for (x, y) in input.chunks(block).zip(out.chunks_mut(block)) {
        drive.process(x, y);
    }
    out
}

fn rms(values: &[f32]) -> f32 {
    (values.iter().map(|v| v * v).sum::<f32>() / values.len() as f32).sqrt()
}

/// A settled processor at 4× with `shape` at `drive`.
fn processor(shape: DriveShape, drive: f32) -> DriveProcessor {
    let mut d = DriveProcessor::new(RATE, Some(Factor::Four), GLIDE);
    d.set_shape(shape);
    d.set_drive(drive);
    d.settle();
    d
}

#[test]
fn every_shape_at_every_setting_stays_within_one_and_finite() {
    let loud = sine(220.0, 3.0, 4096);
    for shape in DriveShape::ALL {
        for drive in [0.0, 0.5, 1.0] {
            for bias in [-1.0, 0.0, 0.7] {
                let mut d = processor(shape, drive);
                d.set_bias(bias);
                d.set_post_tone(1.0);
                d.set_auto_level(false);
                let out = render(&mut d, &loud, 256);
                let bounded = out.iter().all(|v| v.is_finite() && v.abs() <= 1.0);
                assert!(bounded, "{shape:?} drive {drive} bias {bias}");
            }
        }
    }
    let mut d = DriveProcessor::new(RATE, None, GLIDE);
    d.set_drive(f32::NAN);
    let out = render(&mut d, &[f32::NAN, f32::INFINITY, 0.5], 3);
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn auto_level_holds_a_minus_12_dbfs_sine_within_3_db_across_drive() {
    let input = sine(440.0, REFERENCE_PEAK, RATE as usize / 2);
    let reference = rms(&input);
    for shape in &DriveShape::ALL[..5] {
        for step in 0..=10 {
            let drive = step as f32 / 10.0;
            let out = render(&mut processor(*shape, drive), &input, 512);
            let db = 20.0 * (rms(&out[RATE as usize / 8..]) / reference).log10();
            assert!(db.abs() < 3.0, "{shape:?} at drive {drive}: {db:.2} dB");
        }
    }
}

#[test]
fn the_dry_share_lines_up_with_the_driven_one() {
    // At drive 0 a quiet signal is nearly untouched, so half wet and half
    // dry is the input delayed: any misalignment would comb.
    let input = sine(9_000.0, 0.05, 8192);
    let mut d = processor(DriveShape::Soft, 0.0);
    d.set_mix(0.5);
    d.settle();
    let latency = d.latency();
    assert!(latency > 0);
    assert_eq!(DriveProcessor::new(RATE, None, GLIDE).latency(), 0);
    let out = render(&mut d, &input, 128);
    let error = out[4096..]
        .iter()
        .zip(&input[4096 - latency..])
        .map(|(y, x)| (y - x).abs())
        .fold(0.0f32, f32::max);
    assert!(error < 5e-3, "error {error}");
}

/// The largest step between two neighbouring samples.
fn largest_step(values: &[f32]) -> f32 {
    values
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// The largest step `shape` takes on `input` at each of `drives`, settled.
fn steady_step(shape: DriveShape, drives: &[f32], bias: f32, input: &[f32]) -> f32 {
    drives
        .iter()
        .map(|&drive| {
            let mut d = processor(shape, drive);
            d.set_bias(bias);
            d.settle();
            largest_step(&render(&mut d, input, 64)[4800..])
        })
        .fold(0.0, f32::max)
}

#[test]
fn changing_shape_or_drive_mid_tone_never_jumps() {
    // A click is a step bigger than either setting takes on its own: a
    // crossfade or glide moves each sample only a little more.
    let input = sine(110.0, 0.5, RATE as usize / 2);
    for from in DriveShape::ALL {
        for to in DriveShape::ALL {
            let mut d = processor(from, 1.0);
            let mut out = render(&mut d, &input[..9600], 64);
            d.set_shape(to);
            d.set_drive(0.1);
            out.extend(render(&mut d, &input[9600..], 64));
            let steady = steady_step(from, &[1.0, 0.1], 0.0, &input).max(steady_step(
                to,
                &[1.0, 0.1],
                0.0,
                &input,
            ));
            let step = largest_step(&out[4800..]);
            assert!(
                step <= 1.25 * steady + 0.01,
                "{from:?} to {to:?}: step {step}, steady {steady}"
            );
        }
    }
}

#[test]
fn bits_and_rate_ignore_the_bias_so_moving_it_never_steps_the_level() {
    // A bias snapped to the grid would step the level as it moved, on
    // silence and quiet tails most of all.
    let quiet = sine(110.0, 0.01, 9600);
    for shape in [DriveShape::Bits, DriveShape::Rate] {
        let mut still = processor(shape, 1.0);
        let mut moved = processor(shape, 1.0);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for (n, chunk) in quiet.chunks(64).enumerate() {
            moved.set_bias(-1.0 + 2.0 * n as f32 / 150.0);
            a.extend(render(&mut still, chunk, 64));
            b.extend(render(&mut moved, chunk, 64));
        }
        assert_eq!(a, b, "{shape:?}");
    }
}

#[test]
fn asymmetric_drive_leaves_no_offset() {
    let input = sine(100.0, 0.8, 2 * RATE as usize);
    let mut d = processor(DriveShape::Asymmetric, 1.0);
    d.set_bias(0.5);
    d.settle();
    let out = render(&mut d, &input, 512);
    let tail = &out[RATE as usize..];
    let mean = tail.iter().sum::<f32>() / tail.len() as f32;
    assert!(mean.abs() < 1e-3, "mean {mean}");
}

#[test]
fn processing_neither_allocates_nor_frees() {
    let input = sine(300.0, 0.5, MAX_BLOCK);
    let mut output = vec![0.0; MAX_BLOCK];
    let mut d = processor(DriveShape::Fold, 0.5);
    d.set_shape(DriveShape::Bits);
    d.set_pre_tone(0.5);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        d.process(&input, &mut output);
        d.process(&input[..1], &mut output[..1]);
    });
    assert_eq!((allocs, frees), (0, 0));
}

/// Power at the frequencies a 5 kHz tone's aliases land on at 48 kHz (every
/// whole kHz that is no harmonic), over the fundamental's, in dB.
fn alias_ratio_db(out: &[f32]) -> f64 {
    let power = |hz: f64| {
        let (mut re, mut im, mut sum) = (0.0f64, 0.0f64, 0.0f64);
        for (i, &v) in out.iter().enumerate() {
            let p = std::f64::consts::TAU * i as f64 / out.len() as f64;
            let w =
                0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos();
            let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(RATE);
            re += w * f64::from(v) * phase.cos();
            im -= w * f64::from(v) * phase.sin();
            sum += w;
        }
        (2.0 * re.hypot(im) / sum).powi(2)
    };
    let aliases: f64 = (1..=20)
        .filter(|k| k % 5 != 0)
        .map(|k| power(f64::from(k) * 1000.0))
        .sum();
    10.0 * (aliases / power(5000.0)).log10()
}

#[test]
fn four_times_oversampling_holds_hard_clip_aliases_30_db_below_none() {
    for peak in [0.25, 0.5, 1.0] {
        let input = sine(5000.0, peak, RATE as usize / 2);
        let measure = |factor: Option<Factor>| {
            let mut d = DriveProcessor::new(RATE, factor, GLIDE);
            d.set_shape(DriveShape::Hard);
            d.set_drive(1.0);
            d.settle();
            alias_ratio_db(&render(&mut d, &input, 512)[4096..])
        };
        let (none, four) = (measure(None), measure(Some(Factor::Four)));
        assert!(
            none - four >= 30.0,
            "peak {peak}: none {none:.1} dB, 4x {four:.1} dB"
        );
    }
}
