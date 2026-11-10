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

pub fn wavetable() -> usize {
    model_position("wavetable")
}

pub fn chords() -> usize {
    model_position("chords")
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

/// A voice on `position` with `controls` applied, rendered 0.3 s past its
/// start transient.
pub fn settled(position: usize, controls: &[(&str, f32)]) -> Voice {
    let mut v = voice(json!({}));
    v.select(position);
    v.set_all(controls);
    v.render(seconds(0.3));
    v
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

/// Power-weighted mean frequency (Hz) of bins between 20 Hz and 20 kHz.
pub fn centroid(p: &[f64], n: usize) -> f64 {
    let bin_hz = SRF / n as f64;
    let (mut num, mut den) = (0.0, 0.0);
    for (b, v) in p.iter().enumerate() {
        let f = b as f64 * bin_hz;
        if (20.0..=20_000.0).contains(&f) {
            num += f * v;
            den += v;
        }
    }
    num / den.max(1e-300)
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

/// Power in seven octave bands from 125 Hz to 16 kHz, in dB relative to the
/// power from 20 Hz up.
pub fn octave_bands_db(p: &[f64], n: usize) -> Vec<f64> {
    let bin_hz = SRF / n as f64;
    let total: f64 = p
        .iter()
        .enumerate()
        .filter(|(b, _)| *b as f64 * bin_hz >= 20.0)
        .map(|(_, v)| v)
        .sum();
    (0..7)
        .map(|i| {
            let (lo, hi) = (125.0 * 2f64.powi(i), 125.0 * 2f64.powi(i + 1));
            let band: f64 = p
                .iter()
                .enumerate()
                .filter(|(b, _)| (lo..hi).contains(&(*b as f64 * bin_hz)))
                .map(|(_, v)| v)
                .sum();
            db((band + 1e-18 * total) / total.max(1e-300))
        })
        .collect()
}

/// Mean absolute difference of two band vectors, in dB.
pub fn band_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
}
