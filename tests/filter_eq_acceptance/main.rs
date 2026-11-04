//! Acceptance tests for the `filter` module's EQ modes (bell, shelves,
//! notch), its `boost` / `q` / `bypass` / `slope` controls, and the
//! guarantee that `lowpass` / `highpass` / `bandpass` render as before.
//! Written from the spec and the pinned public interface alone; they drive
//! only the public registry and `Module` API at 48 kHz.
//!
//! Magnitude references are the RBJ Audio EQ Cookbook digital responses
//! (`rbj.rs`). Gains are measured as the RMS ratio of a steady sine tone
//! (RMS·√2 amplitude, never sampled peaks), after half a second of settling.
//!
//! Run with `cargo test --release --test filter_eq_acceptance`.

mod bypass;
mod legacy;
mod motion;
mod rbj;
mod response;

use fugue::{GraphModule, ModuleRegistry};
use serde_json::Value;

pub const SR: u32 = 48_000;
pub const BLOCK: usize = 128;
/// Samples in one millisecond.
pub const MS: usize = SR as usize / 1000;
/// Peak amplitude of every test tone and noise signal. Chosen so a +12 dB
/// boost peaks at 1.0, leaving headroom below the ±4 bound.
pub const AMP: f32 = 0.25;

/// The `filter` module built through the public registry, ports by name.
pub struct Filter {
    module: GraphModule,
    audio: usize,
    cutoff: usize,
}

/// A live write: (frame before which it is made, control key, value).
/// Bools are 1.0 / 0.0.
pub type Write = (usize, &'static str, f32);

pub fn filter(config: Value) -> Filter {
    let built = ModuleRegistry::default()
        .build("filter", SR, &config)
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

pub fn sine(len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = std::f64::consts::TAU * freq / f64::from(SR);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
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

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Measured steady-state gain in dB of a filter built from `config` for a
/// sine at `freq`: half a second settles (many time constants even for
/// q = 8 at 100 Hz), the next second is measured as an output/input RMS
/// ratio over the same samples.
pub fn tone_gain_db(config: &Value, freq: f64) -> f64 {
    let (skip, len) = (SR as usize / 2, SR as usize);
    let input = sine(skip + len, freq, AMP);
    let out = filter(config.clone()).run(&input, &[]);
    db(rms(&out[skip..]) / rms(&input[skip..]))
}

/// Largest sample-to-sample step.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

pub fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

/// FNV-1a over the samples' bit patterns.
pub fn hash(x: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in x {
        for byte in v.to_bits().to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}
