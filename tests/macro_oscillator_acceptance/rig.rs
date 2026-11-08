//! Shared rig: building the module, rendering blocks, and pitch measurement.

use super::*;
use fugue::ControlMeta;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 256;
/// The fixed order in which the spec adds models.
pub const MODEL_ORDER: [&str; 7] = [
    "virtual_analog",
    "waveshaping",
    "fm",
    "grain_formant",
    "additive",
    "chords",
    "speech",
];

/// A `macro_oscillator` built through the public registry, with ports
/// resolved by name.
pub struct Voice {
    pub module: GraphModule,
    pub controls: Vec<ControlMeta>,
    /// Model names in option order, as the `model` control reports them.
    pub models: Vec<String>,
    audio: usize,
    aux: usize,
    pub frequency_in: usize,
    pub frequency_mod_in: usize,
    pub trigger_in: usize,
    pub level_in: usize,
    held: Vec<(usize, f32)>,
}

pub fn voice(config: Value) -> Voice {
    let module = ModuleRegistry::default()
        .build("macro_oscillator", SR, &config)
        .unwrap_or_else(|err| panic!("macro_oscillator builds with {config}: {err}"))
        .module;
    let m = module.module();
    let (inputs, outputs) = (m.inputs().to_vec(), m.outputs().to_vec());
    let find = |ports: &[&str], name: &str| {
        ports
            .iter()
            .position(|p| *p == name)
            .unwrap_or_else(|| panic!("port {name} missing, have {ports:?}"))
    };
    let controls = m.controls();
    let models = match controls.iter().find(|c| c.key == "model").map(|c| &c.kind) {
        Some(ControlKind::String { options: Some(o) }) => o.clone(),
        other => panic!("`model` must be a String control with options, got {other:?}"),
    };
    Voice {
        audio: find(&outputs, "audio"),
        aux: find(&outputs, "aux"),
        frequency_in: find(&inputs, "frequency"),
        frequency_mod_in: find(&inputs, "frequency_mod"),
        trigger_in: find(&inputs, "trigger"),
        level_in: find(&inputs, "level"),
        controls,
        models,
        module,
        held: Vec::new(),
    }
}

impl Voice {
    pub fn control(&self, key: &str) -> Option<&ControlMeta> {
        self.controls.iter().find(|c| c.key == key)
    }

    pub fn set(&mut self, key: &str, value: f32) {
        self.module
            .module_mut()
            .set_control(key, value)
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn select(&mut self, position: usize) {
        self.set("model", position as f32);
    }

    /// Connects `port` and holds it at a constant for every later `process`.
    pub fn hold(&mut self, port: usize, value: f32) {
        self.module.module_mut().set_input_connected(port, true);
        self.held.retain(|(p, _)| *p != port);
        self.held.push((port, value));
    }

    pub fn connect(&mut self, port: usize) {
        self.module.module_mut().set_input_connected(port, true);
    }

    pub fn input_block(&mut self, port: usize) -> &mut [f32] {
        self.module.module_mut().input_block_mut(port)
    }

    pub fn process(&mut self, n: usize) {
        let held = self.held.clone();
        let m = self.module.module_mut();
        for (port, value) in held {
            m.input_block_mut(port)[..n].fill(value);
        }
        assert!(m.process(n), "macro_oscillator stays active");
    }

    pub fn audio(&self, n: usize) -> &[f32] {
        &self.module.module().output_block(self.audio)[..n]
    }

    pub fn aux(&self, n: usize) -> &[f32] {
        &self.module.module().output_block(self.aux)[..n]
    }

    /// Renders `frames` frames of `audio` in `block`-sized calls.
    pub fn render_blocks(&mut self, frames: usize, block: usize) -> Vec<f32> {
        assert!((1..=fugue::MAX_BLOCK).contains(&block));
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            let n = (frames - out.len()).min(block);
            self.process(n);
            out.extend_from_slice(self.audio(n));
        }
        out
    }

    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        self.render_blocks(frames, BLOCK)
    }
}

pub fn seconds(s: f64) -> usize {
    (s * SRF) as usize
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Largest |x[n] - x[n-1]|.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Magnitude of the DFT of `xw` at `hz`, by complex rotation.
fn dft_mag(xw: &[f64], hz: f64) -> f64 {
    let (si, co) = (-std::f64::consts::TAU * hz / SRF).sin_cos();
    let (mut zr, mut zi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for &v in xw {
        re += v * zr;
        im += v * zi;
        (zr, zi) = (zr * co - zi * si, zr * si + zi * co);
    }
    re.hypot(im)
}

/// Frequency (Hz) of the Hann-windowed spectral peak within +-50 cents of
/// `hz`: a 1-cent coarse scan, then a 0.05-cent scan around the best bin.
pub fn peak_near(x: &[f32], hz: f64) -> f64 {
    let n = x.len();
    let xw: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            f64::from(*v) * w
        })
        .collect();
    let at = |cents: f64| hz * 2f64.powf(cents / 1200.0);
    let best = |lo: f64, hi: f64, step: f64| {
        let steps = ((hi - lo) / step).round() as i32;
        (0..=steps)
            .map(|i| lo + f64::from(i) * step)
            .map(|c| (c, dft_mag(&xw, at(c))))
            .fold((lo, -1.0), |a, b| if b.1 > a.1 { b } else { a })
            .0
    };
    let coarse = best(-50.0, 50.0, 1.0);
    at(best(coarse - 1.0, coarse + 1.0, 0.05))
}

pub fn cents(found: f64, wanted: f64) -> f64 {
    1200.0 * (found / wanted).log2()
}

/// A voice on model `position` at `hz`, rendered past its start transient.
pub fn settled(position: usize, hz: f32) -> Voice {
    let mut v = voice(json!({}));
    v.select(position);
    v.set("frequency", hz);
    v.render(seconds(0.3));
    v
}
