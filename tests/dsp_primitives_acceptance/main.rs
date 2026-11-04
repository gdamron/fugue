//! Acceptance tests for the DSP primitives (oversampler, DC blocker, one-pole
//! filter), written from the spec alone. Assumptions are marked "Assumption".

mod dc_blocker;
mod no_alloc;
mod one_pole;
mod oversampler;

use std::f64::consts::TAU;

/// Peak amplitude of every test tone.
pub const AMP: f64 = 0.5;

/// A sine of `AMP` at `freq_hz`, sampled at `rate`.
pub fn sine(freq_hz: f64, rate: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| (AMP * (TAU * freq_hz * i as f64 / rate).sin()) as f32)
        .collect()
}

/// Amplitude of the `freq_hz` component of `x` (single-bin DFT under a 4-term
/// Blackman-Harris window, whose sidelobes sit below -92 dB).
pub fn amplitude(x: &[f32], freq_hz: f64, rate: f64) -> f64 {
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let p = TAU * i as f64 / x.len() as f64;
        let w = 0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos();
        let ph = TAU * freq_hz * i as f64 / rate;
        re += w * v as f64 * ph.cos();
        im -= w * v as f64 * ph.sin();
        sum += w;
    }
    2.0 * re.hypot(im) / sum
}

pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Steady-state gain in dB of a per-sample filter at `freq_hz`: the first
/// second is discarded, the next two are measured.
pub fn gain_db(rate: u32, freq_hz: f64, mut filter: impl FnMut(f32) -> f32) -> f64 {
    let r = rate as usize;
    let out: Vec<f32> = sine(freq_hz, rate as f64, 3 * r)
        .into_iter()
        .map(&mut filter)
        .collect();
    db(amplitude(&out[r..], freq_hz, rate as f64) / AMP)
}
