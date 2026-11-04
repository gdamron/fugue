//! Shared rig: building the module, rendering blocks, and measurement helpers.

use super::*;
use std::f64::consts::PI;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const SHAPES: [&str; 6] = ["soft", "hard", "fold", "asymmetric", "bits", "rate"];
/// Shapes that keep level (everything but sample-rate reduction).
pub const LEVEL_KEEPING: [&str; 5] = ["soft", "hard", "fold", "asymmetric", "bits"];

pub struct Rig {
    pub m: Box<dyn Module>,
    pub t: usize,
    pub block: usize,
}

impl Rig {
    pub fn new(config: Value) -> Self {
        Self::at_rate(config, SR)
    }

    pub fn at_rate(config: Value, sample_rate: u32) -> Self {
        let built = ModuleRegistry::default()
            .build("drive", sample_rate, &config)
            .unwrap_or_else(|err| panic!("drive builds with {config}: {err}"));
        let m = match built.module {
            GraphModule::Module(m) => m,
            _ => panic!("drive is a plain module"),
        };
        Rig {
            m,
            t: 0,
            block: 256,
        }
    }

    pub fn set(&mut self, key: &str, value: f32) {
        self.m
            .set_control(key, value)
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn run(&mut self, frames: usize, sig: &dyn Fn(usize) -> f32) -> Vec<f32> {
        self.run_with(frames, sig, &mut |_, _| {})
    }

    /// Renders `frames` frames of `sig(absolute index)`; `hook(index, module)`
    /// runs before each block.
    pub fn run_with(
        &mut self,
        frames: usize,
        sig: &dyn Fn(usize) -> f32,
        hook: &mut dyn FnMut(usize, &mut dyn Module),
    ) -> Vec<f32> {
        self.m.set_input_connected(0, true);
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            let n = self.block.min(frames - out.len());
            hook(self.t, self.m.as_mut());
            for k in 0..n {
                self.m.input_block_mut(0)[k] = sig(self.t + k);
            }
            assert!(self.m.process(n), "drive stays active");
            out.extend_from_slice(&self.m.output_block(0)[..n]);
            self.t += n;
        }
        out
    }
}

pub fn sine(amp: f64, freq: f64) -> impl Fn(usize) -> f32 {
    sine_at(amp, freq, SRF)
}

pub fn sine_at(amp: f64, freq: f64, sample_rate: f64) -> impl Fn(usize) -> f32 {
    move |i| (amp * (2.0 * PI * freq * i as f64 / sample_rate).sin()) as f32
}

pub fn secs(s: f64) -> usize {
    (s * SRF) as usize
}

pub fn mean(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v)).sum::<f64>() / x.len() as f64
}

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

/// Amplitude of the `freq` component over `x` (use whole periods).
pub fn tone_amp(x: &[f32], freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let ph = 2.0 * PI * freq * i as f64 / SRF;
        re += f64::from(*v) * ph.cos();
        im += f64::from(*v) * ph.sin();
    }
    2.0 * re.hypot(im) / x.len() as f64
}

/// Power spectrum (bins 0..=n/2) of `x` under a 4-term Blackman-Harris window.
pub fn power_spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    assert!(n.is_power_of_two());
    let w = |i: usize| {
        let p = 2.0 * PI * i as f64 / n as f64;
        0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos()
    };
    let mut re: Vec<f64> = (0..n).map(|i| f64::from(x[i]) * w(i)).collect();
    let mut im = vec![0.0; n];
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    (0..=n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}

/// Deterministic white noise in [-1, 1).
pub fn noise(seed: u32) -> impl FnMut() -> f32 {
    let mut s = seed;
    move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / (1 << 23) as f32 - 1.0
    }
}
