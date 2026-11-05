//! Shared rig: `granular` driven directly, invention rendering, metrics.

use super::*;

pub const SR: u32 = 48_000;
pub const BLOCK: usize = 64;

pub const GRANULAR: f32 = 0.0;
pub const STRETCH: f32 = 1.0;
pub const LOOPING_DELAY: f32 = 2.0;
pub const MODES: [(&str, f32); 3] = [
    ("granular", GRANULAR),
    ("stretch", STRETCH),
    ("looping_delay", LOOPING_DELAY),
];

/// Seconds to frames.
pub fn s(seconds: f64) -> usize {
    (seconds * f64::from(SR)).round() as usize
}

/// What is patched into the module. Unset inputs stay unpatched.
#[derive(Default, Clone)]
pub struct Feed {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
    pub trigger: Option<Vec<f32>>,
    pub clock: Option<Vec<f32>>,
}

impl Feed {
    pub fn mono(left: Vec<f32>) -> Feed {
        Feed {
            left,
            ..Feed::default()
        }
    }
    pub fn with_trigger(mut self, trigger: Vec<f32>) -> Feed {
        self.trigger = Some(trigger);
        self
    }
    pub fn with_clock(mut self, clock: Vec<f32>) -> Feed {
        self.clock = Some(clock);
        self
    }
}

pub fn try_granular(config: Value) -> Result<GraphModule, String> {
    let built = ModuleRegistry::default().build("granular", SR, &config);
    Ok(built.map_err(|err| err.to_string())?.module)
}

/// A scheduled control write: (frame, key, value). Frames should be
/// multiples of `BLOCK`.
pub type Event = (usize, &'static str, f32);

/// Renders `feed` through a fresh `granular` built from `config`, applying
/// `start` controls before the first sample and `events` at their frames.
/// Returns (left, right).
pub fn render_granular(
    config: Value,
    start: &[(&str, f32)],
    events: &[Event],
    feed: &Feed,
) -> (Vec<f32>, Vec<f32>) {
    let mut g = try_granular(config.clone())
        .unwrap_or_else(|err| panic!("granular builds with {config}: {err}"));
    let m = g.module_mut();
    let port = |name: &str| {
        (m.input_port_index(name)).unwrap_or_else(|| panic!("granular has an input named {name}"))
    };
    let (l, r, t, c) = (
        port("audio_left"),
        port("audio_right"),
        port("trigger"),
        port("clock"),
    );
    assert_eq!(
        [l, r, t, c],
        [0, 1, 2, 3],
        "input order in the interface spec"
    );
    assert_eq!(m.outputs(), ["audio_left", "audio_right"]);
    for &(key, value) in start {
        (m.set_control(key, value)).unwrap_or_else(|e| panic!("accepts {key} = {value}: {e}"));
    }
    m.set_input_connected(l, true);
    m.set_input_connected(r, feed.right.is_some());
    m.set_input_connected(t, feed.trigger.is_some());
    m.set_input_connected(c, feed.clock.is_some());
    let frames = feed.left.len();
    let (mut out_l, mut out_r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut at = 0;
    while at < frames {
        let n = BLOCK.min(frames - at);
        for &(frame, key, value) in events.iter().filter(|e| e.0 >= at && e.0 < at + n) {
            assert_eq!(frame % BLOCK, 0, "events sit on block boundaries");
            (m.set_control(key, value)).unwrap_or_else(|e| panic!("accepts {key}: {e}"));
        }
        let copy = |m: &mut dyn Module, port: usize, src: &[f32]| {
            m.input_block_mut(port)[..n].copy_from_slice(&src[at..at + n]);
        };
        copy(m, l, &feed.left);
        if let Some(x) = &feed.right {
            copy(m, r, x);
        }
        if let Some(x) = &feed.trigger {
            copy(m, t, x);
        }
        if let Some(x) = &feed.clock {
            copy(m, c, x);
        }
        assert!(m.process(n), "granular stays active");
        out_l.extend_from_slice(&m.output_block(0)[..n]);
        out_r.extend_from_slice(&m.output_block(1)[..n]);
        at += n;
    }
    (out_l, out_r)
}

pub fn sine(len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = std::f64::consts::TAU * freq / f64::from(SR);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
        .collect()
}

/// A smooth, band-limited input: two low sines.
pub fn smooth(len: usize) -> Vec<f32> {
    let (a, b) = (sine(len, 110.0, 0.3), sine(len, 247.0, 0.3));
    a.iter().zip(&b).map(|(x, y)| x + y).collect()
}

/// Deterministic white noise in [-amp, amp).
pub fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
    let mut st = seed.max(1);
    (0..len)
        .map(|_| {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            ((st as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
        })
        .collect()
}

/// A gate train: `high` samples at 1.0 starting at each of `edges`.
pub fn gates(len: usize, edges: impl IntoIterator<Item = usize>, high: usize) -> Vec<f32> {
    let mut x = vec![0.0; len];
    for e in edges {
        for v in x.iter_mut().skip(e).take(high) {
            *v = 1.0;
        }
    }
    x
}

/// Largest sample-to-sample step.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Longest run of samples with magnitude at most `eps`.
pub fn longest_quiet_run(x: &[f32], eps: f32) -> usize {
    let (mut best, mut run) = (0, 0);
    for v in x {
        run = if v.abs() <= eps { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// Loads an invention document into a fresh engine.
pub fn engine(doc: Value) -> RenderEngine {
    let mut engine = RenderEngine::new(SR);
    (engine.load_json(&doc.to_string())).unwrap_or_else(|err| panic!("invention loads: {err}"));
    engine
}

/// Renders `frames` (a multiple of 64) and returns the (left, right) outputs.
pub fn render(engine: &mut RenderEngine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).expect("renders");
    let left = out.iter().step_by(2).copied().collect();
    let right = out.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

pub fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// An oscillator (sine, 220 Hz) through `granular` (built from `config`) to
/// the dac, stereo.
pub fn live_doc(config: Value) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220.0 } },
            { "id": "g", "type": "granular", "config": config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("osc", "audio", "g", "audio_left"),
            conn("g", "audio_left", "dac", "audio_left"),
            conn("g", "audio_right", "dac", "audio_right")
        ]
    })
}
