//! Shared rig: building the reverb, rendering blocks, and signal metrics.

use super::*;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 128;
pub const LEFT: usize = 0;
pub const RIGHT: usize = 1;

/// Peak any steady-state test tolerates: a runaway feedback loop passes this
/// within a few seconds, a sanely gain-staged reverb never approaches it.
pub const MAX_PEAK: f32 = 100.0;

/// A reverb built through the factory with the mix held from the first frame:
/// wet 1, dry 0, width 1, then `overrides` on top.
pub fn wet_module(overrides: &[(&str, f64)]) -> Box<dyn Module + Send> {
    let mut cfg = Map::new();
    cfg.insert("wet".into(), json!(1.0));
    cfg.insert("dry".into(), json!(0.0));
    cfg.insert("width".into(), json!(1.0));
    for (key, value) in overrides {
        cfg.insert((*key).into(), json!(*value));
    }
    build(Value::Object(cfg))
}

pub fn build(config: Value) -> Box<dyn Module + Send> {
    match ReverbFactory
        .build(SR, &config)
        .expect("reverb builds")
        .module
    {
        GraphModule::Module(module) => module,
        _ => panic!("reverb is a plain module"),
    }
}

/// Renders `frames` frames in `block`-sized calls. `input(i)` gives the
/// (left, right) input at frame i; the right input is connected only when
/// `stereo`. `hook(i, module)` runs before each block starts at frame i.
pub fn render(
    module: &mut dyn Module,
    frames: usize,
    block: usize,
    mut input: impl FnMut(usize) -> (f32, f32),
    stereo: bool,
    mut hook: impl FnMut(usize, &mut dyn Module),
) -> (Vec<f32>, Vec<f32>) {
    module.set_input_connected(RIGHT, stereo);
    let (mut out_l, mut out_r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        hook(pos, module);
        for k in 0..n {
            let (l, r) = input(pos + k);
            module.input_block_mut(LEFT)[k] = l;
            if stereo {
                module.input_block_mut(RIGHT)[k] = r;
            }
        }
        assert!(module.process(n), "reverb stays active");
        out_l.extend_from_slice(&module.output_block(LEFT)[..n]);
        out_r.extend_from_slice(&module.output_block(RIGHT)[..n]);
        pos += n;
    }
    (out_l, out_r)
}

pub fn no_hook(_: usize, _: &mut dyn Module) {}

/// Deterministic white noise in [-1, 1).
pub struct Noise(pub u32);
impl Noise {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

pub fn secs(s: f64) -> usize {
    (s * SRF) as usize
}

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, &v| m.max(v.abs()))
}

pub fn all_finite(x: &[f32]) -> bool {
    x.iter().all(|v| v.is_finite())
}

/// Zero-lag normalized correlation of two signals.
pub fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (&x, &y) in a.iter().zip(b) {
        ab += x as f64 * y as f64;
        aa += (x as f64).powi(2);
        bb += (y as f64).powi(2);
    }
    ab / (aa * bb).sqrt().max(1e-30)
}
