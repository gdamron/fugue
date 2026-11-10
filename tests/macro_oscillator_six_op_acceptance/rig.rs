//! Shared rig: building the module, rendering, pitch and spectrum measurement.

use super::*;
use fugue::ControlMeta;
use std::f64::consts::TAU;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 256;
/// The host block size the morphing criteria write controls at.
pub const HOST_BLOCK: usize = 64;
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

/// Position of the option called `name` in the `model` control.
pub fn model_position(name: &str) -> usize {
    let models = voice(json!({})).models;
    models
        .iter()
        .position(|m| m == name)
        .unwrap_or_else(|| panic!("model option `{name}` missing, have {models:?}"))
}

pub fn six_op() -> usize {
    model_position("six_op_fm")
}

/// The 32 patch names in index order.
pub const PATCH_NAMES: [&str; 32] = [
    "glass piano",
    "soft tine keys",
    "bright tine keys",
    "reed keys",
    "funky pluck keys",
    "quill keys",
    "toy piano",
    "music box",
    "tubular bell",
    "glockenspiel",
    "vibraphone",
    "marimba",
    "kalimba",
    "steel pan",
    "gong",
    "crystal bell",
    "solid bass",
    "slap bass",
    "rubber bass",
    "sub pluck",
    "harp",
    "plucked silk",
    "nylon guitar",
    "twang",
    "drawbar organ",
    "reed organ",
    "warm pad",
    "glass pad",
    "brass section",
    "flute",
    "reed lead",
    "metal drone",
];

/// `harmonics` at the centre of patch `i`'s band.
pub fn patch_harmonics(i: usize) -> f32 {
    (i as f32 + 0.5) / 32.0
}

/// A `six_op_fm` voice on patch `patch` at 220 Hz with `controls` applied
/// after, trigger unpatched (droning), rendered 0.3 s past its start.
pub fn drone(patch: usize, controls: &[(&str, f32)]) -> Voice {
    let mut v = voice(json!({}));
    v.select(six_op());
    v.set_all(&[("frequency", 220.0), ("harmonics", patch_harmonics(patch))]);
    v.set_all(controls);
    v.render(seconds(0.3));
    v
}

/// A fresh `six_op_fm` voice on `patch` at 220 Hz with the trigger patched.
pub fn triggered(patch: usize, controls: &[(&str, f32)]) -> Voice {
    let mut v = voice(json!({}));
    v.select(six_op());
    v.set_all(&[("frequency", 220.0), ("harmonics", patch_harmonics(patch))]);
    v.set_all(controls);
    let port = v.trigger_in;
    v.connect(port);
    v
}

/// A trigger signal of `frames` frames, high over each `(from, to)` range.
pub fn gate(frames: usize, highs: &[(usize, usize)]) -> Vec<f32> {
    let mut g = vec![0.0; frames];
    for (from, to) in highs {
        g[*from..(*to).min(frames)].fill(1.0);
    }
    g
}

/// Renders `trigger.len()` frames in `HOST_BLOCK`-frame blocks with the
/// trigger patched and fed from `trigger`; `before_block(voice, frame)` runs
/// before each block (for control writes). Returns (audio, aux).
pub fn drive(
    v: &mut Voice,
    trigger: &[f32],
    mut before_block: impl FnMut(&mut Voice, usize),
) -> (Vec<f32>, Vec<f32>) {
    let port = v.trigger_in;
    v.connect(port);
    let (mut audio, mut aux) = (Vec::new(), Vec::new());
    let mut at = 0;
    while at < trigger.len() {
        let n = (trigger.len() - at).min(HOST_BLOCK);
        before_block(v, at);
        v.input_block(port)[..n].copy_from_slice(&trigger[at..at + n]);
        v.process(n);
        audio.extend_from_slice(v.audio(n));
        aux.extend_from_slice(v.aux(n));
        at += n;
    }
    (audio, aux)
}

/// `drive` without control writes, audio only.
pub fn play(v: &mut Voice, trigger: &[f32]) -> Vec<f32> {
    drive(v, trigger, |_, _| {}).0
}

impl Voice {
    pub fn set(&mut self, key: &str, value: f32) {
        self.module
            .module_mut()
            .set_control(key, value)
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn set_all(&mut self, controls: &[(&str, f32)]) {
        for (key, value) in controls {
            self.set(key, *value);
        }
    }

    pub fn select(&mut self, position: usize) {
        self.set("model", position as f32);
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

    /// Renders `frames` frames in `block`-sized calls: (audio, aux).
    pub fn render_both_blocks(&mut self, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
        assert!((1..=fugue::MAX_BLOCK).contains(&block));
        let (mut a, mut x) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        while a.len() < frames {
            let n = (frames - a.len()).min(block);
            self.process(n);
            a.extend_from_slice(self.audio(n));
            x.extend_from_slice(self.aux(n));
        }
        (a, x)
    }

    pub fn render_both(&mut self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        self.render_both_blocks(frames, BLOCK)
    }

    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        self.render_both(frames).0
    }

    /// Renders `frames` frames while a write of `key` goes linearly from
    /// `from` to `to`, once at the start of every `HOST_BLOCK`-frame block.
    pub fn sweep(&mut self, key: &str, from: f32, to: f32, frames: usize) -> Vec<f32> {
        let blocks = frames / HOST_BLOCK;
        let mut out = Vec::with_capacity(blocks * HOST_BLOCK);
        for i in 0..blocks {
            self.set(key, from + (to - from) * i as f32 / (blocks - 1) as f32);
            self.process(HOST_BLOCK);
            out.extend_from_slice(self.audio(HOST_BLOCK));
        }
        out
    }
}

pub fn seconds(s: f64) -> usize {
    (s * SRF) as usize
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Largest |x[n] - x[n-1]|.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

pub fn cents(found: f64, wanted: f64) -> f64 {
    1200.0 * (found / wanted).log2()
}

pub fn db(ratio: f64) -> f64 {
    10.0 * ratio.max(1e-300).log10()
}

// ---------------------------------------------------------------- spectra

/// In-place radix-2 FFT; the length is a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two());
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
        let (sin, cos) = (-TAU / len as f64).sin_cos();
        let (wr, wi) = (cos, sin);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// 4-term Blackman-Harris window value at `i` of `n`.
fn blackman_harris(i: usize, n: usize) -> f64 {
    let t = TAU * i as f64 / n as f64;
    0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
}

/// Power spectrum (bins 0..=n/2) of the first `n` samples of `x` under a
/// Blackman-Harris window, zero-padded to `n * pad`; `n * pad` is a power of
/// two. Bin `b` sits at `b * SR / (n * pad)` Hz.
pub fn power_spectrum_padded(x: &[f32], n: usize, pad: usize) -> Vec<f64> {
    let size = n * pad;
    let mut re = vec![0.0; size];
    let mut im = vec![0.0; size];
    for i in 0..n {
        re[i] = f64::from(x[i]) * blackman_harris(i, n);
    }
    fft(&mut re, &mut im);
    (0..=size / 2)
        .map(|b| re[b] * re[b] + im[b] * im[b])
        .collect()
}

/// Power spectrum of the last `n` (a power of two) samples of `x`.
pub fn power_spectrum(x: &[f32], n: usize) -> Vec<f64> {
    power_spectrum_padded(&x[x.len() - n..], n, 1)
}

/// Magnitude of the windowed DFT of `xw` at `hz`, by complex rotation.
fn dft_mag(xw: &[f64], hz: f64) -> f64 {
    let (si, co) = (-TAU * hz / SRF).sin_cos();
    let (mut zr, mut zi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for &v in xw {
        re += v * zr;
        im += v * zi;
        (zr, zi) = (zr * co - zi * si, zr * si + zi * co);
    }
    re.hypot(im)
}

/// Frequency (Hz) of the spectral peak within +-50 cents of `hz`, from the
/// first power-of-two samples of `x`: a padded FFT finds the peak bin, then a ternary search of the exact windowed DFT
/// refines it.
pub fn peak_near(x: &[f32], hz: f64) -> f64 {
    let n = if x.len().is_power_of_two() {
        x.len()
    } else {
        x.len().next_power_of_two() / 2
    };
    let pad = ((1usize << 18) / n).max(1);
    let size = n * pad;
    let spectrum = power_spectrum_padded(&x[..n], n, pad);
    let bin_hz = SRF / size as f64;
    let (lo, hi) = (
        hz * 2f64.powf(-50.0 / 1200.0),
        hz * 2f64.powf(50.0 / 1200.0),
    );
    let (b0, b1) = ((lo / bin_hz) as usize, (hi / bin_hz).ceil() as usize);
    let best = (b0..=b1)
        .max_by(|a, b| spectrum[*a].total_cmp(&spectrum[*b]))
        .unwrap();
    let xw: Vec<f64> = (0..n)
        .map(|i| f64::from(x[i]) * blackman_harris(i, n))
        .collect();
    let (mut a, mut b) = ((best as f64 - 1.0) * bin_hz, (best as f64 + 1.0) * bin_hz);
    for _ in 0..30 {
        let (m1, m2) = (a + (b - a) / 3.0, b - (b - a) / 3.0);
        if dft_mag(&xw, m1) < dft_mag(&xw, m2) {
            a = m1;
        } else {
            b = m2;
        }
    }
    (a + b) / 2.0
}

/// Power above `cut` Hz over the power from 20 Hz up, in dB.
pub fn high_share_db(p: &[f64], n: usize, cut: f64) -> f64 {
    let bin_hz = SRF / n as f64;
    let (mut high, mut all) = (0.0, 0.0);
    for (b, v) in p.iter().enumerate() {
        let f = b as f64 * bin_hz;
        if f >= 20.0 {
            all += v;
            if f >= cut {
                high += v;
            }
        }
    }
    db(high / all.max(1e-300))
}
