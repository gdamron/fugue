//! Shared rig: building the module on a named engine, driving the trigger
//! input, finding the half-decay point of a hit, and pitch measurement.

use super::*;
use fugue::ControlMeta;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 256;

/// The seven models that exist before the engines under test, in order.
pub const EXISTING: [&str; 7] = [
    "virtual_analog",
    "waveshaping",
    "fm",
    "grain_formant",
    "additive",
    "chords",
    "speech",
];
/// The engines under test, in the order the spec appends them.
pub const NEW: [&str; 7] = [
    "string",
    "modal",
    "bass_drum",
    "snare_drum",
    "hi_hat",
    "filtered_noise",
    "particle_noise",
];

/// Length of a trigger pulse in frames (about 1.3 ms at 48 kHz).
pub const PULSE: usize = 64;
/// Frame at which the first trigger of a hit rises, so the module has run a
/// few blocks (with the trigger low) before it.
pub const T0: usize = 480;

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
    /// Frames rendered through `render_driven` so far.
    pub frame: usize,
}

/// Builds a `macro_oscillator` from `config`.
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
        frame: 0,
    }
}

/// A voice built with `{"model": name}` in its config, with the model's
/// position checked against the `model` control.
pub fn engine(name: &str) -> Voice {
    engine_with(name, json!({}))
}

/// As `engine`, with extra config keys merged in.
pub fn engine_with(name: &str, extra: Value) -> Voice {
    let mut config = extra;
    config["model"] = json!(name);
    let v = voice(config);
    let position = v
        .models
        .iter()
        .position(|m| m == name)
        .unwrap_or_else(|| panic!("model {name} is not among the options {:?}", v.models));
    let got = v.module.module().get_control("model");
    assert_eq!(
        got,
        Ok(position as f32),
        "config model {name} reads back as position {position}"
    );
    v
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

    /// Renders `frames` frames of `audio` with the trigger input left
    /// unpatched.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            let n = (frames - out.len()).min(BLOCK);
            self.process(n);
            out.extend_from_slice(self.audio(n));
        }
        out
    }

    /// Connects the trigger input and renders `frames` frames, the trigger at
    /// absolute frame `i` (counted over every `render_driven` call on this
    /// voice) being `trigger(i)`. Returns `(audio, aux)`.
    pub fn render_driven(
        &mut self,
        frames: usize,
        trigger: impl Fn(usize) -> f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let port = self.trigger_in;
        self.connect(port);
        let (mut audio, mut aux) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            for i in 0..n {
                let t = trigger(self.frame + i);
                self.input_block(port)[i] = t;
            }
            self.process(n);
            audio.extend_from_slice(self.audio(n));
            aux.extend_from_slice(self.aux(n));
            self.frame += n;
            done += n;
        }
        (audio, aux)
    }
}

/// A trigger signal that is 1 for `PULSE` frames from each frame in `at`,
/// else 0.
pub fn pulses(at: &[usize]) -> impl Fn(usize) -> f32 {
    let at = at.to_vec();
    move |i| {
        if at.iter().any(|&t| i >= t && i < t + PULSE) {
            1.0
        } else {
            0.0
        }
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

/// Peak of each `window`-frame window of `x`, advancing by `hop`.
pub fn windowed_peaks(x: &[f32], window: usize, hop: usize) -> Vec<f32> {
    (0..)
        .map(|k| k * hop)
        .take_while(|s| s + window <= x.len())
        .map(|s| peak(&x[s..s + window]))
        .collect()
}

/// -90 dBFS as a peak amplitude.
pub const MINUS_90_DB: f32 = 3.162_277_7e-5;

/// The frame, counted from the start of the render, at which a single hit
/// from silence has decayed to half of its peak: the first 50 ms window
/// (advancing 10 ms) after the loudest one whose peak is at or below half the
/// loudest window's peak. `None` when that does not happen within `x`.
pub fn half_decay_frame(x: &[f32]) -> Option<usize> {
    const HOP: usize = 480;
    let peaks = windowed_peaks(x, 2400, HOP);
    let (loudest, top) =
        peaks
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |a, (i, p)| if *p > a.1 { (i, *p) } else { a });
    assert!(top > 1e-3, "the hit is silent (peak {top})");
    peaks
        .iter()
        .enumerate()
        .skip(loudest + 1)
        .find(|(_, p)| **p <= 0.5 * top)
        .map(|(k, _)| k * HOP)
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
