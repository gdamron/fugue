//! Shared rig: a module built through the public registry, driven block by
//! block with named inputs, plus signal helpers.

use super::*;

pub const SR: u32 = 48_000;

pub struct Unit {
    module: GraphModule,
    pub block: usize,
    pub t: usize,
}

/// An input: port name and its signal at absolute frame `t`.
pub type Input<'a> = (&'a str, &'a dyn Fn(usize) -> f32);

pub fn try_build(ty: &str, config: Value) -> Result<Unit, String> {
    let built = ModuleRegistry::default()
        .build(ty, SR, &config)
        .map_err(|err| err.to_string())?;
    Ok(Unit {
        module: built.module,
        block: 128,
        t: 0,
    })
}

pub fn build(ty: &str, config: Value) -> Unit {
    try_build(ty, config.clone()).unwrap_or_else(|err| panic!("{ty} builds with {config}: {err}"))
}

impl Unit {
    pub fn m(&self) -> &dyn Module {
        self.module.module()
    }

    pub fn set(&mut self, key: &str, value: f32) {
        (self.module.module_mut().set_control(key, value))
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    /// Renders `frames` frames; only the listed inputs are connected. Returns
    /// one vector per output port, in port order. `hook(t, unit)` runs before
    /// each block.
    pub fn run_with(
        &mut self,
        frames: usize,
        inputs: &[Input],
        hook: &mut dyn FnMut(usize, &mut Unit),
    ) -> Vec<Vec<f32>> {
        let ports: Vec<(usize, &dyn Fn(usize) -> f32)> = (inputs.iter())
            .map(|(name, sig)| {
                let idx = self.m().input_port_index(name);
                (idx.unwrap_or_else(|| panic!("has an input {name}")), *sig)
            })
            .collect();
        for &(idx, _) in &ports {
            self.module.module_mut().set_input_connected(idx, true);
        }
        let mut out = vec![Vec::with_capacity(frames); self.m().outputs().len()];
        let mut done = 0;
        while done < frames {
            let n = self.block.min(frames - done);
            hook(self.t, self);
            let (t, m) = (self.t, self.module.module_mut());
            for &(idx, sig) in &ports {
                for (k, x) in m.input_block_mut(idx)[..n].iter_mut().enumerate() {
                    *x = sig(t + k);
                }
            }
            assert!(m.process(n), "module stays active");
            for (o, buf) in out.iter_mut().enumerate() {
                buf.extend_from_slice(&m.output_block(o)[..n]);
            }
            self.t += n;
            done += n;
        }
        out
    }

    pub fn run(&mut self, frames: usize, inputs: &[Input]) -> Vec<Vec<f32>> {
        self.run_with(frames, inputs, &mut |_, _| {})
    }
}

pub fn secs(s: f64) -> usize {
    (s * f64::from(SR)).round() as usize
}

pub fn db_to_amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

pub fn sine(amp: f64, freq: f64) -> impl Fn(usize) -> f32 {
    move |t| (amp * (std::f64::consts::TAU * freq * t as f64 / f64::from(SR)).sin()) as f32
}

/// Deterministic white noise in [-amp, amp), the same for the same `t`.
pub fn noise(amp: f32, seed: u32) -> impl Fn(usize) -> f32 {
    move |t| {
        let mut s = (t as u32).wrapping_mul(2_654_435_761) ^ seed;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        ((s >> 8) as f32 / (1 << 23) as f32 - 1.0) * amp
    }
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}
