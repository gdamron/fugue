//! Shared rig: the phaser built through the public registry and driven block
//! by block, plus test signals and metrics.

use super::*;

pub const SR: u32 = 48_000;

pub struct Unit {
    module: GraphModule,
    pub block: usize,
    pub t: usize,
}

/// An input: port name and its whole signal.
pub type Input<'a> = (&'a str, &'a [f32]);

pub fn try_build(config: Value) -> Result<Unit, String> {
    let built = ModuleRegistry::default()
        .build("phaser", SR, &config)
        .map_err(|err| err.to_string())?;
    Ok(Unit {
        module: built.module,
        block: 128,
        t: 0,
    })
}

pub fn build(config: Value) -> Unit {
    try_build(config.clone()).unwrap_or_else(|err| panic!("phaser builds with {config}: {err}"))
}

impl Unit {
    pub fn m(&self) -> &dyn Module {
        self.module.module()
    }

    pub fn set(&mut self, key: &str, value: f32) {
        (self.module.module_mut().set_control(key, value))
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn meta(&self, key: &str) -> ControlMeta {
        let found = self.m().controls().into_iter().find(|c| c.key == key);
        found.unwrap_or_else(|| panic!("declares a {key} control"))
    }

    /// Renders as many frames as the shortest input has; only the listed
    /// inputs are connected. Returns the (left, right) outputs.
    /// `hook(t, unit)` runs before each block starting at frame `t`.
    pub fn run_with(
        &mut self,
        inputs: &[Input],
        hook: &mut dyn FnMut(usize, &mut Unit),
    ) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(self.m().outputs(), ["audio_left", "audio_right"]);
        let frames = inputs.iter().map(|(_, x)| x.len()).min().unwrap();
        let ports: Vec<(usize, &[f32])> = (inputs.iter())
            .map(|(name, sig)| {
                let idx = self.m().input_port_index(name);
                (idx.unwrap_or_else(|| panic!("has an input {name}")), *sig)
            })
            .collect();
        for &(idx, _) in &ports {
            self.module.module_mut().set_input_connected(idx, true);
        }
        let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let mut done = 0;
        while done < frames {
            let n = self.block.min(frames - done);
            hook(self.t, self);
            let m = self.module.module_mut();
            for &(idx, sig) in &ports {
                m.input_block_mut(idx)[..n].copy_from_slice(&sig[done..done + n]);
            }
            assert!(m.process(n), "phaser stays active");
            left.extend_from_slice(&m.output_block(0)[..n]);
            right.extend_from_slice(&m.output_block(1)[..n]);
            self.t += n;
            done += n;
        }
        (left, right)
    }

    /// A mono source on `audio_left` only (`audio_right` unpatched).
    pub fn run_mono(&mut self, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        self.run_with(&[("audio_left", x)], &mut |_, _| {})
    }

    /// A mono source on `audio_left`, with per-block control writes.
    pub fn run_hooked(
        &mut self,
        x: &[f32],
        mut hook: impl FnMut(usize, &mut Unit),
    ) -> (Vec<f32>, Vec<f32>) {
        self.run_with(&[("audio_left", x)], &mut hook)
    }
}

pub fn secs(s: f64) -> usize {
    (s * f64::from(SR)).round() as usize
}

pub fn sine(len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = std::f64::consts::TAU * freq / f64::from(SR);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
        .collect()
}

/// A smooth pad: two detuned copies of a 1/n harmonic stack (12 partials) on
/// `root`, scaled so its peak is `amp`.
pub fn pad(len: usize, root: f64, amp: f32) -> Vec<f32> {
    let tau = std::f64::consts::TAU / f64::from(SR);
    let x: Vec<f32> = (0..len)
        .map(|n| {
            let t = n as f64;
            let stack = |f0: f64| {
                (1..=12)
                    .map(|k| (tau * f0 * f64::from(k) * t).sin() / f64::from(k))
                    .sum::<f64>()
            };
            (stack(root) + stack(root * 1.007)) as f32
        })
        .collect();
    let top = peak(&x);
    x.iter().map(|v| v / top * amp).collect()
}

/// Deterministic white noise in [-amp, amp].
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

/// Full-scale square wave of the given period in samples.
pub fn square(len: usize, period: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|n| if n % period < period / 2 { amp } else { -amp })
        .collect()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

/// Largest step into a sample that starts a `block`-sized call.
pub fn max_edge_step(x: &[f32], block: usize) -> f32 {
    (block..x.len())
        .step_by(block)
        .fold(0.0, |m, n| m.max((x[n] - x[n - 1]).abs()))
}

pub fn rms(x: &[f32]) -> f64 {
    let sum: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / x.len() as f64).sqrt()
}

/// RMS of the difference of two equal-length signals.
pub fn rms_diff(a: &[f32], b: &[f32]) -> f64 {
    let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    rms(&d)
}

pub fn mono_fold(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| (a + b) / 2.0).collect()
}

pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

pub fn assert_finite(x: &[f32], what: &str) {
    assert!(x.iter().all(|v| v.is_finite()), "{what} stays finite");
}
