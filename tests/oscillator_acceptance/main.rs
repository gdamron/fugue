//! Acceptance tests for the sine table and the band-limited oscillator,
//! written from the spec alone.

mod aliasing;
mod no_alloc;
mod sine_table;
mod sync;

use std::f64::consts::TAU;

pub const RATE: f64 = 48_000.0;
/// Samples analysed (a power of two) and warm-up samples skipped before them.
pub const N: usize = 1 << 14;
pub const SKIP: usize = 100;

/// In-place radix-2 FFT.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wi, wr) = (-TAU * k as f64 / len as f64).sin_cos();
                let (a, b) = (s + k, s + k + len / 2);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len <<= 1;
    }
}

/// `(aliased, total)` power in 0-20 kHz of `x[SKIP..SKIP + N]` under a 4-term
/// Blackman-Harris window. "Aliased" is every bin farther than 6 bins (the
/// window's main lobe is +-4) from the nearest multiple of `f0_hz`, DC included.
pub fn alias_power(x: &[f32], f0_hz: f64) -> (f64, f64) {
    let mut re: Vec<f64> = x[SKIP..SKIP + N]
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = TAU * i as f64 / N as f64;
            let w =
                0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos();
            v as f64 * w
        })
        .collect();
    let mut im = vec![0.0; N];
    fft(&mut re, &mut im);
    let bin_hz = RATE / N as f64;
    let (mut aliased, mut total) = (0.0, 0.0);
    for b in 0..=(20_000.0 / bin_hz) as usize {
        let hz = b as f64 * bin_hz;
        let power = re[b] * re[b] + im[b] * im[b];
        total += power;
        if (hz - (hz / f0_hz).round() * f0_hz).abs() > 6.0 * bin_hz {
            aliased += power;
        }
    }
    (aliased, total)
}

pub fn db(power_ratio: f64) -> f64 {
    10.0 * power_ratio.log10()
}
