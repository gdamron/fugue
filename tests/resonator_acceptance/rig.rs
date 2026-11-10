//! Shared rig: building the module, driving its inputs, rendering blocks, and
//! pitch and level measurement.

use super::*;
use fugue::ControlMeta;

pub const SR: u32 = 48_000;
pub const SRF: f64 = SR as f64;
pub const BLOCK: usize = 256;
/// -90 dBFS as a linear amplitude.
pub const MINUS_90_DB: f32 = 3.162_277_7e-5;
/// Frames the strum pulse stays high after a strike.
const STRUM_HIGH_FRAMES: usize = 32;

/// A `resonator` built through the public registry, with ports resolved by
/// name. A strike is queued with `strike` or `impulse` and lands at the first
/// frame of the next `process` call.
pub struct Reso {
    pub module: GraphModule,
    pub controls: Vec<ControlMeta>,
    /// Model names in option order, as the `model` control reports them.
    pub models: Vec<String>,
    pub audio_in: usize,
    pub frequency_in: usize,
    pub strum_in: usize,
    out_audio: usize,
    out_odd: usize,
    out_even: usize,
    audio_connected: bool,
    frequency_held: Option<f32>,
    pending_strum: bool,
    pending_impulse: bool,
}

/// Three output streams rendered together.
#[derive(Default, Clone)]
pub struct Rendered {
    pub audio: Vec<f32>,
    pub odd: Vec<f32>,
    pub even: Vec<f32>,
}

impl Rendered {
    pub fn each(&self) -> [(&'static str, &[f32]); 3] {
        [
            ("audio", &self.audio),
            ("odd", &self.odd),
            ("even", &self.even),
        ]
    }
}

pub fn try_build(config: &Value) -> Result<GraphModule, String> {
    ModuleRegistry::default()
        .build("resonator", SR, config)
        .map(|built| built.module)
        .map_err(|err| err.to_string())
}

pub fn reso(config: Value) -> Reso {
    let module =
        try_build(&config).unwrap_or_else(|err| panic!("resonator builds with {config}: {err}"));
    let m = module.module();
    let (inputs, outputs) = (m.inputs().to_vec(), m.outputs().to_vec());
    let find = |ports: &[&str], name: &str| {
        ports
            .iter()
            .position(|p| *p == name)
            .unwrap_or_else(|| panic!("port {name} missing, have {ports:?}"))
    };
    let controls = m.controls();
    // Assumption: `model` is reported as a String control with options, as
    // every other choice control in the library is.
    let models = match controls.iter().find(|c| c.key == "model").map(|c| &c.kind) {
        Some(ControlKind::String { options: Some(o) }) => o.clone(),
        other => panic!("`model` must be a String control with options, got {other:?}"),
    };
    let mut r = Reso {
        out_audio: find(&outputs, "audio"),
        out_odd: find(&outputs, "odd"),
        out_even: find(&outputs, "even"),
        audio_in: find(&inputs, "audio"),
        frequency_in: find(&inputs, "frequency"),
        strum_in: find(&inputs, "strum"),
        controls,
        models,
        module,
        audio_connected: false,
        frequency_held: None,
        pending_strum: false,
        pending_impulse: false,
    };
    let strum = r.strum_in;
    r.module.module_mut().set_input_connected(strum, true);
    r
}

impl Reso {
    pub fn set(&mut self, key: &str, value: f32) {
        self.module
            .module_mut()
            .set_control(key, value)
            .unwrap_or_else(|err| panic!("{key} accepts {value}: {err}"));
    }

    pub fn select(&mut self, position: usize) {
        self.set("model", position as f32);
    }

    /// Connects `audio`, which turns the internal exciter off.
    pub fn connect_audio(&mut self) {
        self.audio_connected = true;
        let port = self.audio_in;
        self.module.module_mut().set_input_connected(port, true);
    }

    pub fn connect(&mut self, port: usize) {
        self.module.module_mut().set_input_connected(port, true);
    }

    /// Connects `frequency` and holds it at `hz` for every later `process`.
    pub fn hold_frequency(&mut self, hz: f32) {
        let port = self.frequency_in;
        self.module.module_mut().set_input_connected(port, true);
        self.frequency_held = Some(hz);
    }

    pub fn input_block(&mut self, port: usize) -> &mut [f32] {
        self.module.module_mut().input_block_mut(port)
    }

    /// Queues a strum rising edge at the first frame of the next block.
    pub fn strike(&mut self) {
        self.pending_strum = true;
    }

    /// Queues a full-scale one-sample impulse on `audio` at the first frame
    /// of the next block (connects `audio` if it is not).
    pub fn impulse(&mut self) {
        if !self.audio_connected {
            self.connect_audio();
        }
        self.pending_impulse = true;
    }

    pub fn process(&mut self, n: usize) {
        let (strum, audio, freq) = (self.strum_in, self.audio_in, self.frequency_in);
        let high = if self.pending_strum {
            n.min(STRUM_HIGH_FRAMES)
        } else {
            0
        };
        self.pending_strum = false;
        let impulse = std::mem::take(&mut self.pending_impulse);
        let (audio_connected, held) = (self.audio_connected, self.frequency_held);
        let m = self.module.module_mut();
        let block = m.input_block_mut(strum);
        block[..n].fill(0.0);
        block[..high].fill(1.0);
        if audio_connected {
            let block = m.input_block_mut(audio);
            block[..n].fill(0.0);
            if impulse {
                block[0] = 1.0;
            }
        }
        if let Some(hz) = held {
            m.input_block_mut(freq)[..n].fill(hz);
        }
        assert!(m.process(n), "resonator stays active");
    }

    pub fn audio(&self, n: usize) -> &[f32] {
        &self.module.module().output_block(self.out_audio)[..n]
    }

    pub fn odd(&self, n: usize) -> &[f32] {
        &self.module.module().output_block(self.out_odd)[..n]
    }

    pub fn even(&self, n: usize) -> &[f32] {
        &self.module.module().output_block(self.out_even)[..n]
    }

    /// Renders `frames` frames in `block`-sized calls, calling `each` with
    /// the module after every call.
    pub fn run(&mut self, frames: usize, block: usize, mut each: impl FnMut(&Reso, usize)) {
        assert!((1..=fugue::MAX_BLOCK).contains(&block));
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(block);
            self.process(n);
            each(self, n);
            done += n;
        }
    }

    /// Renders all three outputs.
    pub fn render_all(&mut self, frames: usize, block: usize) -> Rendered {
        let mut out = Rendered::default();
        self.run(frames, block, |r, n| {
            out.audio.extend_from_slice(r.audio(n));
            out.odd.extend_from_slice(r.odd(n));
            out.even.extend_from_slice(r.even(n));
        });
        out
    }

    /// Renders `audio` only.
    pub fn render_blocks(&mut self, frames: usize, block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        self.run(frames, block, |r, n| out.extend_from_slice(r.audio(n)));
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

/// Root mean square of `x`.
pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt() as f32
}

/// Hann-windowed DFT magnitude at `hz`, by complex rotation.
pub fn dft_mag_hann(x: &[f32], hz: f64) -> f64 {
    let n = x.len();
    let (si, co) = (-std::f64::consts::TAU * hz / SRF).sin_cos();
    let (mut zr, mut zi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        let v = f64::from(v) * w;
        re += v * zr;
        im += v * zi;
        (zr, zi) = (zr * co - zi * si, zr * si + zi * co);
    }
    re.hypot(im)
}

/// Frequency (Hz) of the Hann-windowed spectral peak within +-50 cents of
/// `hz`: a 2-cent coarse scan, then a 0.05-cent scan around the best point.
pub fn peak_near(x: &[f32], hz: f64) -> f64 {
    let at = |cents: f64| hz * 2f64.powf(cents / 1200.0);
    let best = |lo: f64, hi: f64, step: f64| {
        let steps = ((hi - lo) / step).round() as i32;
        (0..=steps)
            .map(|i| lo + f64::from(i) * step)
            .map(|c| (c, dft_mag_hann(x, at(c))))
            .fold((lo, -1.0), |a, b| if b.1 > a.1 { b } else { a })
            .0
    };
    let coarse = best(-50.0, 50.0, 2.0);
    at(best(coarse - 2.0, coarse + 2.0, 0.05))
}

pub fn cents(found: f64, wanted: f64) -> f64 {
    1200.0 * (found / wanted).log2()
}

/// A resonator at `hz` on model `position` with `damping`, glides finished,
/// nothing struck yet.
pub fn ready(position: usize, hz: f32, damping: f32) -> Reso {
    let mut r = reso(json!({}));
    r.select(position);
    r.set("frequency", hz);
    r.set("damping", damping);
    r.render(seconds(0.3));
    r
}
