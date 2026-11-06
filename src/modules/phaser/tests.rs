//! Tests for the phaser.

use super::*;

const RATE_HZ: u32 = 48_000;

/// A sine at `frequency` Hz and `level`, sample `n`.
fn tone(n: usize, frequency: f32, level: f32) -> f32 {
    level * (2.0 * PI * frequency * n as f32 / RATE_HZ as f32).sin()
}

/// Renders `frames` of `source` into the left input, in blocks of `block`,
/// calling `before(phaser, frame)` ahead of each block. Returns both sides.
fn render(
    phaser: &mut Phaser,
    frames: usize,
    block: usize,
    source: impl Fn(usize) -> f32,
    mut before: impl FnMut(&mut Phaser, usize),
) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut start = 0;
    while start < frames {
        let n = block.min(frames - start);
        before(phaser, start);
        for i in 0..n {
            phaser.input_block_mut(0)[i] = source(start + i);
        }
        phaser.process(n);
        left.extend_from_slice(&phaser.output_block(0)[..n]);
        right.extend_from_slice(&phaser.output_block(1)[..n]);
        start += n;
    }
    (left, right)
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn mix_zero_passes_the_input_exactly() {
    let mut phaser = Phaser::new(RATE_HZ);
    phaser.set_control("mix", 0.0).unwrap();
    phaser.mix.settle();
    let (left, right) = render(&mut phaser, 4_800, 64, |n| tone(n, 440.0, 0.8), |_, _| {});
    for (n, (&l, &r)) in left.iter().zip(&right).enumerate() {
        assert_eq!(l, tone(n, 440.0, 0.8));
        assert_eq!(r, l, "the right copies the left while unpatched");
    }
}

#[test]
fn six_stages_notch_a_sine_at_the_centre() {
    let mut phaser = Phaser::new(RATE_HZ);
    for (key, value) in [("depth", 0.0), ("frequency", 700.0), ("feedback", 0.0)] {
        phaser.set_control(key, value).unwrap();
    }
    let (left, _) = render(&mut phaser, 24_000, 128, |n| tone(n, 700.0, 0.5), |_, _| {});
    let tail = &left[12_000..];
    let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 0.01, "notch leaves {peak}");
}

/// Rate, depth and feedback rewritten at every block edge, as fast as a
/// scheduler can: no step outgrows what the tone itself moves.
#[test]
fn fast_sweeps_have_no_steps_at_block_edges() {
    for block in [1, 16, 64, 100] {
        let mut phaser = Phaser::new(RATE_HZ);
        let source = |n| tone(n, 220.0, 0.5);
        let (left, right) = render(&mut phaser, RATE_HZ as usize, block, source, |p, at| {
            let t = at as f32 / RATE_HZ as f32;
            let edge = (at / block) % 2 == 0;
            p.set_control("rate", if edge { 10.0 } else { 0.01 })
                .unwrap();
            p.set_control("depth", if edge { 1.0 } else { 0.0 })
                .unwrap();
            // Within ±0.5: past that a resonance lifts the tone itself.
            p.set_control("feedback", 0.5 * (2.0 * PI * 3.0 * t).sin())
                .unwrap();
        });
        let dry: Vec<f32> = (0..RATE_HZ as usize).map(source).collect();
        let bound = 1.5 * largest_step(&dry);
        for side in [&left, &right] {
            let step = largest_step(side);
            assert!(step <= bound, "block {block}: step {step} over {bound}");
        }
    }
}

#[test]
fn a_stage_count_change_crossfades() {
    let mut phaser = Phaser::new(RATE_HZ);
    phaser.set_control("depth", 0.0).unwrap();
    let source = |n| tone(n, 1_000.0, 0.5);
    let (left, _) = render(&mut phaser, 24_000, 64, source, |p, at| {
        if at == 9_600 {
            p.set_control("stage_count", 12.0).unwrap();
        } else if at == 14_400 {
            p.set_control("stage_count", 4.0).unwrap();
        }
    });
    let dry: Vec<f32> = (0..24_000).map(source).collect();
    let step = largest_step(&left);
    assert!(step <= 1.5 * largest_step(&dry), "step {step}");
}

#[test]
fn full_feedback_on_full_scale_noise_stays_within_one() {
    for (feedback, stages) in [(0.95, 4.0), (-0.95, 12.0), (0.95, 12.0)] {
        let mut phaser = Phaser::new(RATE_HZ);
        phaser.set_control("feedback", feedback).unwrap();
        phaser.set_control("stage_count", stages).unwrap();
        phaser.set_control("mix", 1.0).unwrap();
        let mut rng = crate::dsp::Rng::new(7);
        let noise: Vec<f32> = (0..96_000).map(|_| rng.bipolar()).collect();
        let (left, right) = render(&mut phaser, noise.len(), 128, |n| noise[n], |_, _| {});
        for v in left.iter().chain(&right) {
            assert!(v.is_finite() && v.abs() <= 1.0, "{feedback}/{stages}: {v}");
        }
    }
}

#[test]
fn a_quarter_note_sweep_repeats_every_half_second_at_120_bpm() {
    let mut phaser = Phaser::new(RATE_HZ);
    phaser.set_control("division", 3.0).unwrap();
    phaser.set_control("rate", 7.0).unwrap();
    let source = |n| tone(n, 1_000.0, 0.5);
    let (left, _) = render(&mut phaser, 72_000, 128, source, |_, _| {});
    for n in 30_000..36_000 {
        assert!((left[n] - left[n + 24_000]).abs() < 2e-3, "frame {n}");
    }
}
