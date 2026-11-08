//! Shared rig: building the module, rendering blocks, and spectrum helpers.

use super::*;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 256;

/// A noise module built through the public registry, with its output ports
/// resolved by name.
pub struct Noise {
    module: GraphModule,
    white: usize,
    pink: usize,
}

/// Builds `noise` with `config` through the default registry.
pub fn noise(config: Value) -> Noise {
    let built = ModuleRegistry::default()
        .build("noise", SR, &config)
        .unwrap_or_else(|err| panic!("noise builds with {config}: {err}"));
    let module = built.module;
    let outputs = module.module().outputs().to_vec();
    let find = |name: &str| {
        outputs
            .iter()
            .position(|port| *port == name)
            .unwrap_or_else(|| panic!("noise has an output named {name}, has {outputs:?}"))
    };
    let (white, pink) = (find("white"), find("pink"));
    Noise {
        module,
        white,
        pink,
    }
}

/// Noise seeded with `seed`.
pub fn seeded(seed: u64) -> Noise {
    noise(json!({ "seed": seed }))
}

impl Noise {
    /// Renders `frames` frames in `BLOCK`-sized calls (or `block`-sized, with
    /// [`Noise::render_blocks`]); returns (white, pink).
    pub fn render(&mut self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        self.render_blocks(frames, BLOCK)
    }

    pub fn render_blocks(&mut self, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
        assert!((1..=fugue::MAX_BLOCK).contains(&block));
        let mut white = Vec::with_capacity(frames);
        let mut pink = Vec::with_capacity(frames);
        let mut left = frames;
        while left > 0 {
            let n = left.min(block);
            let module = self.module.module_mut();
            assert!(module.process(n), "noise stays active");
            white.extend_from_slice(&module.output_block(self.white)[..n]);
            pink.extend_from_slice(&module.output_block(self.pink)[..n]);
            left -= n;
        }
        (white, pink)
    }

    /// Renders and discards `frames` frames (lets a control glide settle).
    pub fn run(&mut self, frames: usize) {
        let _ = self.render(frames);
    }

    pub fn set_level(&mut self, level: f32) {
        self.module
            .module_mut()
            .set_control("level", level)
            .unwrap_or_else(|err| panic!("level accepts {level}: {err}"));
    }

    pub fn level(&self) -> f32 {
        self.module
            .module()
            .get_control("level")
            .expect("level is readable")
    }

    pub fn module(&self) -> &dyn Module {
        self.module.module()
    }
}

pub fn seconds(s: f64) -> usize {
    (s * SRF) as usize
}

pub fn mean(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v)).sum::<f64>() / x.len() as f64
}

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// In-place radix-2 complex FFT. `re.len()` must be a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
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

pub const SEGMENT: usize = 32768;

/// Welch-averaged one-sided power spectrum: Hann window, 50% overlap,
/// `SEGMENT`-point segments. Bin `k` sits at `k * SR / SEGMENT` Hz.
pub fn welch_psd(x: &[f32]) -> Vec<f64> {
    let window: Vec<f64> = (0..SEGMENT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / SEGMENT as f64).cos())
        .collect();
    let mut psd = vec![0.0; SEGMENT / 2 + 1];
    let mut segments = 0;
    let mut start = 0;
    while start + SEGMENT <= x.len() {
        let mut re: Vec<f64> = (0..SEGMENT)
            .map(|i| f64::from(x[start + i]) * window[i])
            .collect();
        let mut im = vec![0.0; SEGMENT];
        fft(&mut re, &mut im);
        for k in 0..=SEGMENT / 2 {
            psd[k] += re[k] * re[k] + im[k] * im[k];
        }
        segments += 1;
        start += SEGMENT / 2;
    }
    assert!(segments > 8, "enough segments to average");
    psd.iter().map(|p| p / f64::from(segments)).collect()
}

/// Mean power (in dB) per third-octave band from 50 Hz to 10 kHz, as
/// (log2 of the band's geometric centre in Hz, dB). Mean power per bin, not
/// summed power, so the result is a spectral density: white is flat, pink
/// falls 3 dB per octave.
pub fn third_octave_density_db(psd: &[f64]) -> Vec<(f64, f64)> {
    let hz_per_bin = SRF / SEGMENT as f64;
    let mut bands = Vec::new();
    for k in 0..22 {
        let lo = 50.0 * 2f64.powf(f64::from(k) / 3.0);
        let hi = (50.0 * 2f64.powf(f64::from(k + 1) / 3.0)).min(10_000.0);
        let bins: Vec<f64> = (1..psd.len())
            .filter(|b| {
                let f = *b as f64 * hz_per_bin;
                f >= lo && f < hi
            })
            .map(|b| psd[b])
            .collect();
        assert!(bins.len() >= 4, "band {lo}-{hi} Hz has bins to average");
        let power = bins.iter().sum::<f64>() / bins.len() as f64;
        bands.push(((lo * hi).sqrt().log2(), 10.0 * power.log10()));
    }
    bands
}

/// Least-squares slope in dB per octave of `(log2 f, dB)` points.
pub fn slope_db_per_octave(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let num: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    num / den
}
