//! Shared rig: building the module, driving inputs, and measurement helpers.

use super::*;
use fugue::ControlMeta;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;

/// The four outputs, `contour.0` to `contour.3`.
pub type Out = [Vec<f32>; 4];

pub struct Contour {
    pub module: GraphModule,
    pub controls: Vec<ControlMeta>,
    /// Frames per `process` call.
    pub block: usize,
    outs: [usize; 4],
    pos: usize,
}

pub fn contour(config: Value) -> Contour {
    let module = ModuleRegistry::default()
        .build("contour", SR, &config)
        .unwrap_or_else(|err| panic!("contour builds with {config}: {err}"))
        .module;
    let m = module.module();
    let outs = ["contour.0", "contour.1", "contour.2", "contour.3"].map(|name| {
        m.outputs()
            .iter()
            .position(|p| *p == name)
            .unwrap_or_else(|| panic!("output {name} missing, have {:?}", m.outputs()))
    });
    let controls = m.controls();
    Contour {
        module,
        controls,
        block: 256,
        outs,
        pos: 0,
    }
}

/// Position of option `name` in choice control `key`.
pub fn option_position(controls: &[ControlMeta], key: &str, name: &str) -> f32 {
    let meta = controls
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("control {key} is declared"));
    let ControlKind::String { options: Some(o) } = &meta.kind else {
        panic!("{key} is a choice control, got {:?}", meta.kind);
    };
    o.iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{key} has option {name}, has {o:?}")) as f32
}

impl Contour {
    pub fn set(&mut self, key: &str, value: f32) {
        self.module
            .module_mut()
            .set_control(key, value)
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn choose(&mut self, key: &str, name: &str) {
        let position = option_position(&self.controls, key, name);
        self.set(key, position);
    }

    /// Renders `frames` frames, feeding each named input from `inputs` at the
    /// module's running sample position (so signals span several calls).
    pub fn render(&mut self, frames: usize, inputs: &[(&str, &[f32])]) -> Out {
        let m = self.module.module_mut();
        let ports: Vec<(usize, &[f32])> = inputs
            .iter()
            .map(|(name, signal)| {
                let port = m
                    .inputs()
                    .iter()
                    .position(|p| p == name)
                    .unwrap_or_else(|| panic!("input {name} missing, have {:?}", m.inputs()));
                (port, *signal)
            })
            .collect();
        for (port, _) in &ports {
            m.set_input_connected(*port, true);
        }
        let mut out: Out = Default::default();
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.block);
            for (port, signal) in &ports {
                m.input_block_mut(*port)[..n].copy_from_slice(&signal[self.pos..self.pos + n]);
            }
            assert!(m.process(n), "contour stays active");
            for k in 0..4 {
                out[k].extend_from_slice(&m.output_block(self.outs[k])[..n]);
            }
            self.pos += n;
            left -= n;
        }
        out
    }
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn f32(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// 1 from `from` up to (not including) `to`, else 0.
pub fn gate(len: usize, from: usize, to: usize) -> Vec<f32> {
    (0..len).map(|i| f32::from(i >= from && i < to)).collect()
}

/// Gates `width` samples long every `period` samples, the first at sample 0.
pub fn pulse_train(len: usize, period: usize, width: usize) -> Vec<f32> {
    (0..len).map(|i| f32::from(i % period < width)).collect()
}

/// Indices where `x` rises through 0.5 (from a start of 0).
pub fn rising(x: &[f32]) -> Vec<usize> {
    let mut prev = 0.0;
    let mut edges = Vec::new();
    for (i, v) in x.iter().enumerate() {
        if *v > 0.5 && prev <= 0.5 {
            edges.push(i);
        }
        prev = *v;
    }
    edges
}

/// Indices (>= 1) where a ramp restarts: a local minimum after a fall.
pub fn cycle_starts(x: &[f32]) -> Vec<usize> {
    (1..x.len() - 1)
        .filter(|&i| x[i - 1] > x[i] && x[i + 1] >= x[i])
        .collect()
}

/// Largest |x[n] - x[n-1]|.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// In-place radix-2 complex FFT. `re.len()` must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
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
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}
