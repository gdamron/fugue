//! Shared rig: the delay driven directly, invention rendering, signal metrics.

use super::*;

pub const SR: u32 = 48_000;
pub const BLOCK: usize = 128;

/// The `delay` atom built through the public registry, ports resolved by name.
pub struct Delay {
    module: GraphModule,
    audio: usize,
    time_mod: usize,
    bpm: usize,
    /// While `Some`, written to the `bpm` input every block (and connected).
    pub bpm_in: Option<f32>,
    /// Constant written to the `time_mod` input every block.
    pub mod_in: f32,
}

pub fn try_delay(config: Value) -> Result<Delay, String> {
    let built = ModuleRegistry::default()
        .build("delay", SR, &config)
        .map_err(|err| err.to_string())?;
    let module = built.module;
    let port = |name: &str| {
        module
            .module()
            .input_port_index(name)
            .unwrap_or_else(|| panic!("delay has an input named {name}"))
    };
    let (audio, time_mod, bpm) = (port("audio"), port("time_mod"), port("bpm"));
    assert_eq!(module.module().output_port_index("audio"), Some(0));
    Ok(Delay {
        module,
        audio,
        time_mod,
        bpm,
        bpm_in: None,
        mod_in: 0.0,
    })
}

pub fn delay(config: Value) -> Delay {
    try_delay(config.clone()).unwrap_or_else(|err| panic!("delay builds with {config}: {err}"))
}

impl Delay {
    pub fn set(&mut self, key: &str, value: f32) {
        (self.module.module_mut().set_control(key, value))
            .unwrap_or_else(|err| panic!("delay accepts {key} = {value}: {err}"));
    }

    /// Patches the `bpm` input with a constant tempo.
    pub fn connect_bpm(&mut self, bpm: f32) {
        self.bpm_in = Some(bpm);
        self.module.module_mut().set_input_connected(self.bpm, true);
    }

    /// Renders `input` in `block`-sized calls; `hook(frame, self)` runs
    /// before the block starting at `frame`.
    pub fn run(
        &mut self,
        input: &[f32],
        block: usize,
        mut hook: impl FnMut(usize, &mut Delay),
    ) -> Vec<f32> {
        self.module
            .module_mut()
            .set_input_connected(self.audio, true);
        let mut out = Vec::with_capacity(input.len());
        for (i, chunk) in input.chunks(block).enumerate() {
            hook(i * block, self);
            let n = chunk.len();
            let (audio, time_mod, bpm) = (self.audio, self.time_mod, self.bpm);
            let (mod_in, bpm_in) = (self.mod_in, self.bpm_in);
            let m = self.module.module_mut();
            m.input_block_mut(audio)[..n].copy_from_slice(chunk);
            m.input_block_mut(time_mod)[..n].fill(mod_in);
            if let Some(b) = bpm_in {
                m.input_block_mut(bpm)[..n].fill(b);
            }
            assert!(m.process(n), "delay stays active");
            out.extend_from_slice(&m.output_block(0)[..n]);
        }
        out
    }

    pub fn render(&mut self, input: &[f32]) -> Vec<f32> {
        self.run(input, BLOCK, |_, _| {})
    }
}

/// Silence with the given (position, amplitude) impulses.
pub fn impulses(len: usize, at: &[(usize, f32)]) -> Vec<f32> {
    let mut x = vec![0.0; len];
    for &(pos, amp) in at {
        x[pos] = amp;
    }
    x
}

/// Every sample above `eps`, as (position, value).
pub fn nonzero(x: &[f32]) -> Vec<(usize, f32)> {
    let hits = x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4);
    hits.map(|(i, &v)| (i, v)).collect()
}

pub fn sine(len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = std::f64::consts::TAU * freq / f64::from(SR);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
        .collect()
}

/// Deterministic white noise in [-amp, amp).
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

/// Largest sample-to-sample step.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Fractional positions where `x` crosses `level` upward (linear interpolation).
pub fn rising_crossings(x: &[f32], level: f32) -> Vec<f64> {
    let mut found = Vec::new();
    for n in 1..x.len() {
        if x[n - 1] < level && x[n] >= level {
            let frac = f64::from(level - x[n - 1]) / f64::from(x[n] - x[n - 1]);
            found.push((n - 1) as f64 + frac);
        }
    }
    found
}

/// Loads an invention document into a fresh engine.
pub fn engine(doc: Value) -> RenderEngine {
    let mut engine = RenderEngine::new(SR);
    (engine.load_json(&doc.to_string())).unwrap_or_else(|err| panic!("invention loads: {err}"));
    engine
}

/// Renders `frames` (a multiple of 64, the engine's block) and returns the
/// (left, right) outputs.
pub fn render(engine: &mut RenderEngine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).expect("renders");
    let left = out.iter().step_by(2).copied().collect();
    let right = out.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

pub fn set(engine: &RenderEngine, module: &str, key: &str, value: impl Into<ControlValue>) {
    (engine.set_control(module, key, value.into()))
        .unwrap_or_else(|err| panic!("{module}.{key} accepts the value: {err}"));
}

pub fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// An invention that sends `source` to the left output (dry) and through the
/// delay `d` (built from `delay_config`) to the right.
pub fn tap_doc(source: Value, source_port: &str, delay_config: Value) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            source,
            { "id": "d", "type": "delay", "config": delay_config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("src", source_port, "dac", "audio_left"),
            conn("src", source_port, "d", "audio"),
            conn("d", "audio", "dac", "audio_right")
        ]
    })
}

pub fn noise_source(level: f64) -> Value {
    json!({ "id": "src", "type": "noise", "config": { "seed": 7, "level": level } })
}

/// Asserts that the only samples above 1e-4 are `want` (position, amplitude),
/// amplitudes within 1e-3. Positions are exact.
pub fn assert_hits(out: &[f32], want: &[(usize, f32)]) {
    let got = nonzero(out);
    let same = got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g.0 == w.0 && (g.1 - w.1).abs() < 1e-3);
    assert!(same, "echoes {got:?} differ from expected {want:?}");
}

/// Indices of the first sample at or above `level` after one below it (the
/// signal starts below).
pub fn rising_edges(x: &[f32], level: f32) -> Vec<usize> {
    let mut prev = 0.0;
    let mut found = Vec::new();
    for (n, &v) in x.iter().enumerate() {
        if prev < level && v >= level {
            found.push(n);
        }
        prev = v;
    }
    found
}
