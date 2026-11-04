use super::*;
use std::f64::consts::TAU;

const RATE: u32 = 48_000;

/// A sine at `hz` and `rate`, `frames` long.
fn sine(hz: f64, rate: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (TAU * hz * n as f64 / rate).sin() as f32)
        .collect()
}

/// The amplitude of the `hz` component of `signal` at `rate`, through a
/// four-term Blackman-Harris window (sidelobes 92 dB down).
fn amplitude(signal: &[f32], hz: f64, rate: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im, mut gain) = (0.0, 0.0, 0.0);
    for (i, &x) in signal.iter().enumerate() {
        let t = TAU * i as f64 / (n - 1.0);
        let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
        let phase = TAU * hz * i as f64 / rate;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
        gain += w;
    }
    2.0 * (re * re + im * im).sqrt() / gain
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-20).log10()
}

/// Runs `signal` through `stage` in blocks of `block` input frames.
fn in_blocks(
    signal: &[f32],
    block: usize,
    ratio_out: f64,
    mut stage: impl FnMut(&[f32], &mut [f32]),
) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in signal.chunks(block) {
        let mut buffer = vec![0.0; (chunk.len() as f64 * ratio_out) as usize];
        stage(chunk, &mut buffer);
        out.extend_from_slice(&buffer);
    }
    out
}

/// The part of `signal` after the filters have filled.
fn settled(signal: &[f32]) -> &[f32] {
    &signal[512..]
}

#[test]
fn a_round_trip_is_flat_and_delayed_by_its_latency() {
    for factor in [Factor::Two, Factor::Four] {
        let mut over = Oversampler::new(factor, RATE);
        let hz = 1_000.0;
        let input = sine(hz, f64::from(RATE), 8_192);
        let mut output = input.clone();
        for chunk in output.chunks_mut(256) {
            over.process(chunk, |_| {});
        }
        let latency = f64::from(over.latency());
        let late: Vec<f32> = (0..8_192)
            .map(|n| (TAU * hz * (n as f64 - latency) / f64::from(RATE)).sin() as f32)
            .collect();
        for n in 512..8_192 {
            assert!(
                (output[n] - late[n]).abs() < 2e-3,
                "{factor:?} frame {n}: {} vs {}",
                output[n],
                late[n]
            );
        }
    }
}

#[test]
fn low_sample_rates_keep_a_band_below_nyquist() {
    let mut over = Oversampler::new(Factor::Two, 22_050);
    let input = sine(5_000.0, 22_050.0, 8_192);
    let up = in_blocks(&input, MAX_BLOCK, 2.0, |i, o| over.upsample(i, o));
    let gain = db(amplitude(settled(&up), 5_000.0, 44_100.0));
    assert!(gain.abs() <= 0.1, "{gain:.3} dB");
}
