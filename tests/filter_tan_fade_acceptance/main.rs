//! Acceptance tests for the `filter` module's cutoff accuracy at high
//! frequencies and its crossfade when `filter_type` changes while audio
//! plays. Written from the spec and the public `Module` API alone.
//!
//! - `cutoff.rs`: lowpass / highpass / bandpass place the cutoff within
//!   0.1% of the set frequency from 20 Hz up to 0.45 × the sample rate, at
//!   44.1, 48 and 96 kHz, measured from the rendered response.
//! - `fade.rs`: a live `filter_type` change crossfades over about 10 ms
//!   without a step larger than either response's own.
//! - `steady.rs`: with no type switch, every type renders as before at low
//!   cutoffs (within -90 dBFS of references computed here).
//!
//! Gains are fitted amplitudes of a steady sine (least squares on the sine
//! and cosine at the tone's frequency over many cycles), never sampled peaks.
//!
//! Run with `cargo test --release --test filter_tan_fade_acceptance`.

mod cutoff;
mod fade;
mod reference;
mod steady;

use fugue::{GraphModule, ModuleRegistry};
use serde_json::Value;
use std::f64::consts::TAU;

pub const BLOCK: usize = 128;
/// The sample rates the cutoff contract covers.
pub const RATES: [u32; 3] = [44_100, 48_000, 96_000];
/// -90 dBFS as a linear sample difference.
pub const MINUS_90_DBFS: f32 = 3.162_277_7e-5;

/// The `filter_type` options in their control order.
pub const TYPES: [&str; 7] = [
    "lowpass",
    "highpass",
    "bandpass",
    "bell",
    "lowshelf",
    "highshelf",
    "notch",
];

/// The position of `name` among the `filter_type` options, as `set_control`
/// takes it.
pub fn type_position(name: &str) -> f32 {
    TYPES
        .iter()
        .position(|t| *t == name)
        .expect("a filter type") as f32
}

/// The `filter` module built through the public registry, ports by name.
pub struct Filter {
    module: GraphModule,
    audio: usize,
    cutoff: usize,
}

/// A live write: (frame before which it is made, control key, value).
pub type Write = (usize, &'static str, f32);

pub fn filter(sr: u32, config: &Value) -> Filter {
    let built = ModuleRegistry::default()
        .build("filter", sr, config)
        .unwrap_or_else(|err| panic!("filter builds with {config}: {err}"));
    let module = built.module;
    let port = |name: &str| {
        module
            .module()
            .input_port_index(name)
            .unwrap_or_else(|| panic!("filter has an input named {name}"))
    };
    let (audio, cutoff) = (port("audio"), port("cutoff"));
    assert_eq!(module.module().output_port_index("audio"), Some(0));
    let mut f = Filter {
        module,
        audio,
        cutoff,
    };
    f.module.module_mut().set_input_connected(audio, true);
    f
}

impl Filter {
    pub fn set(&mut self, key: &str, value: f32) {
        (self.module.module_mut().set_control(key, value))
            .unwrap_or_else(|err| panic!("filter accepts {key} = {value}: {err}"));
    }

    /// Renders `input` in blocks of at most `BLOCK`, split so each write in
    /// `writes` lands exactly before its frame. With `cutoff_in`, the
    /// `cutoff` input is connected and fed it sample by sample (Hz).
    pub fn run_with(
        &mut self,
        input: &[f32],
        writes: &[Write],
        cutoff_in: Option<&[f32]>,
    ) -> Vec<f32> {
        if cutoff_in.is_some() {
            let idx = self.cutoff;
            self.module.module_mut().set_input_connected(idx, true);
        }
        let mut out = Vec::with_capacity(input.len());
        let mut pos = 0;
        while pos < input.len() {
            for &(at, key, value) in writes {
                if at == pos {
                    self.set(key, value);
                }
            }
            let next_write = (writes.iter().map(|w| w.0))
                .filter(|&at| at > pos)
                .min()
                .unwrap_or(usize::MAX);
            let end = (pos + BLOCK).min(input.len()).min(next_write);
            let n = end - pos;
            let (audio, cutoff) = (self.audio, self.cutoff);
            let m = self.module.module_mut();
            m.input_block_mut(audio)[..n].copy_from_slice(&input[pos..end]);
            if let Some(c) = cutoff_in {
                m.input_block_mut(cutoff)[..n].copy_from_slice(&c[pos..end]);
            }
            m.process(n);
            out.extend_from_slice(&m.output_block(0)[..n]);
            pos = end;
        }
        out
    }

    pub fn run(&mut self, input: &[f32], writes: &[Write]) -> Vec<f32> {
        self.run_with(input, writes, None)
    }
}

/// Renders `input` through a fresh filter built from `config`.
pub fn render(sr: u32, config: &Value, input: &[f32]) -> Vec<f32> {
    filter(sr, config).run(input, &[])
}

pub fn sine(sr: u32, len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = TAU * freq / f64::from(sr);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
        .collect()
}

/// A band-limited sawtooth summed from its harmonics below `top` Hz, so its
/// reset is a smooth ramp rather than a one-sample jump.
pub fn saw(sr: u32, len: usize, f0: f64, top: f64, amp: f32) -> Vec<f32> {
    let harmonics = (top / f0) as usize;
    let w = TAU * f0 / f64::from(sr);
    let scale = f64::from(amp) * 2.0 / std::f64::consts::PI;
    (0..len)
        .map(|n| {
            let p = w * n as f64;
            let sum: f64 = (1..=harmonics)
                .map(|k| (k as f64 * p).sin() / k as f64)
                .sum();
            (scale * sum) as f32
        })
        .collect()
}

/// Deterministic white noise in [-amp, amp).
pub fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            ((s as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
        })
        .collect()
}

/// Least-squares fit of `x` (starting at sample `start` of the render) to
/// a·sin + b·cos at `freq`. Exact for a pure sinusoid over any window.
pub fn fit(sr: u32, x: &[f32], start: usize, freq: f64) -> (f64, f64) {
    let w = TAU * freq / f64::from(sr);
    let (mut ss, mut cc, mut sc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let p = w * (start + i) as f64;
        let (s, c) = p.sin_cos();
        let v = f64::from(v);
        ss += s * s;
        cc += c * c;
        sc += s * c;
        xs += v * s;
        xc += v * c;
    }
    let det = ss * cc - sc * sc;
    ((xs * cc - xc * sc) / det, (xc * ss - xs * sc) / det)
}

/// Largest sample-to-sample step.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

pub fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}
