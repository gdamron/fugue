//! Acceptance tests for click-free live `cutoff` / `resonance` writes on the
//! `filter` module, written from the behaviour spec alone. They drive only
//! the public API (`ModuleRegistry`, `Module`, `RenderEngine`) and measure
//! what is heard: one-sample steps (clicks), how soon a write arrives,
//! bit-exactness where nothing should change.
//!
//! Spec summary (48 kHz):
//! 1. A live write of `cutoff` (20 Hz <-> 20 kHz) or `resonance` (0 <-> 0.9)
//!    on a running filter fed a 0.5-amplitude 1 kHz sine never makes the
//!    output step by more than 1.5x the dry sine's largest step.
//! 2. A live write arrives within about 5-10 ms.
//! 3. Writes made before the filter has processed any audio land at once:
//!    bit-identical to a filter built at the written value.
//! 4. A filter whose controls are never written live renders bit-exactly as
//!    before the change (hashes captured on the previous main).
//! 5. Audio-rate modulation through the `cutoff` input is not smoothed.
//!
//! Run with `cargo test --release --test filter_glide_acceptance`.

use fugue::{ControlValue, GraphModule, ModuleRegistry, RenderEngine};
use serde_json::{json, Value};

const SR: u32 = 48_000;
const BLOCK: usize = 128;
/// Samples in one millisecond.
const MS: usize = SR as usize / 1000;
/// A 1 kHz sine at 48 kHz peaks at sample 12 of every 48-sample period.
const PEAK_OFFSET: usize = 12;

/// The `filter` module built through the public registry, ports by name.
struct Filter {
    module: GraphModule,
    audio: usize,
    cutoff: usize,
}

fn filter(config: Value) -> Filter {
    let built = ModuleRegistry::default()
        .build("filter", SR, &config)
        .unwrap_or_else(|err| panic!("filter builds with {config}: {err}"));
    let module = built.module;
    let port = |name: &str| {
        module
            .module()
            .input_port_index(name)
            .unwrap_or_else(|| panic!("filter has an input named {name}"))
    };
    let (audio, cutoff) = (port("audio"), port("cutoff"));
    assert_eq!(module.module().output_port_index("audio"), Some(0));
    let mut f = Filter {
        module,
        audio,
        cutoff,
    };
    f.module.module_mut().set_input_connected(audio, true);
    f
}

/// A live write: (frame before which it is made, control key, value).
type Write = (usize, &'static str, f32);

impl Filter {
    fn set(&mut self, key: &str, value: f32) {
        (self.module.module_mut().set_control(key, value))
            .unwrap_or_else(|err| panic!("filter accepts {key} = {value}: {err}"));
    }

    /// Renders `input` in blocks of at most `BLOCK`, split so that each write
    /// in `writes` is made exactly before its frame. When `cutoff_in` is
    /// given, the `cutoff` input is connected and fed it sample by sample.
    fn run_with(&mut self, input: &[f32], writes: &[Write], cutoff_in: Option<&[f32]>) -> Vec<f32> {
        if cutoff_in.is_some() {
            let idx = self.cutoff;
            self.module.module_mut().set_input_connected(idx, true);
        }
        let mut out = Vec::with_capacity(input.len());
        let mut pos = 0;
        while pos < input.len() {
            for &(at, key, value) in writes {
                if at == pos {
                    self.set(key, value);
                }
            }
            let next_write = writes
                .iter()
                .map(|w| w.0)
                .filter(|&at| at > pos)
                .min()
                .unwrap_or(usize::MAX);
            let end = (pos + BLOCK).min(input.len()).min(next_write);
            let n = end - pos;
            let (audio, cutoff) = (self.audio, self.cutoff);
            let m = self.module.module_mut();
            m.input_block_mut(audio)[..n].copy_from_slice(&input[pos..end]);
            if let Some(c) = cutoff_in {
                m.input_block_mut(cutoff)[..n].copy_from_slice(&c[pos..end]);
            }
            m.process(n);
            out.extend_from_slice(&m.output_block(0)[..n]);
            pos = end;
        }
        out
    }

    fn run(&mut self, input: &[f32], writes: &[Write]) -> Vec<f32> {
        self.run_with(input, writes, None)
    }
}

fn sine(len: usize, freq: f64, amp: f32) -> Vec<f32> {
    let w = std::f64::consts::TAU * freq / f64::from(SR);
    (0..len)
        .map(|n| amp * (w * n as f64).sin() as f32)
        .collect()
}

/// Deterministic white noise in [-amp, amp).
fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
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
fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

/// FNV-1a over the samples' bit patterns.
fn hash(x: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in x {
        for byte in v.to_bits().to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The 1 kHz, 0.5-amplitude test sine, one second plus `extra` long.
fn test_sine(extra: usize) -> Vec<f32> {
    sine(SR as usize + extra, 1000.0, 0.5)
}

/// Asserts the click criterion: the output's largest step is no more than
/// 1.5x the dry input's, and the output is audible (`min_peak`).
fn assert_no_click(label: &str, input: &[f32], out: &[f32], min_peak: f32) {
    assert!(
        peak(out) >= min_peak,
        "{label}: the sine is heard after the write (peak {})",
        peak(out)
    );
    let (wet, dry) = (max_step(out), max_step(input));
    assert!(
        wet <= 1.5 * dry,
        "{label}: largest step {wet} exceeds 1.5 x dry {dry} ({:.2}x)",
        wet / dry
    );
}

// ---------------------------------------------------------------- criterion 1

/// Opening 20 Hz -> 20 kHz, written at several arbitrary samples.
#[test]
fn opening_cutoff_at_arbitrary_samples_does_not_click() {
    for at in [SR as usize + 1, SR as usize + 7, SR as usize + 29, 50_001] {
        let input = test_sine(SR as usize / 2);
        let mut f = filter(json!({ "filter_type": "lowpass", "cutoff": 20.0, "resonance": 0.0 }));
        let out = f.run(&input, &[(at, "cutoff", 20000.0)]);
        assert_no_click(&format!("open at {at}"), &input, &out, 0.45);
    }
}

/// Closing 20 kHz -> 20 Hz, at a peak and at an arbitrary sample.
#[test]
fn closing_cutoff_does_not_click() {
    for at in [SR as usize + PEAK_OFFSET, SR as usize + 31] {
        let input = test_sine(SR as usize / 2);
        let mut f =
            filter(json!({ "filter_type": "lowpass", "cutoff": 20000.0, "resonance": 0.0 }));
        let out = f.run(&input, &[(at, "cutoff", 20.0)]);
        assert_no_click(&format!("close at {at}"), &input, &out, 0.45);
        let tail = &out[out.len() - 4800..];
        assert!(
            peak(tail) < 0.05,
            "the filter closed (tail peak {})",
            peak(tail)
        );
    }
}

/// Resonance 0 -> 0.9 and back while running, written at eight phases of
/// the sine (one every 6 samples). Assumption: the cutoff is set to 4 kHz,
/// two octaves above the sine, so the resonant steady state itself stays
/// inside the bound (about 1.06x dry) and only a step from the write could
/// exceed it.
#[test]
fn resonance_writes_do_not_click() {
    let s = SR as usize;
    for offset in (0..48).step_by(6) {
        let input = test_sine(SR as usize);
        let writes = [
            (s + offset, "resonance", 0.9),
            (s + 9_600 + offset, "resonance", 0.0),
        ];
        let mut f = filter(json!({ "filter_type": "lowpass", "cutoff": 4000.0, "resonance": 0.0 }));
        let out = f.run(&input, &writes);
        let label = format!("resonance 0 -> 0.9 -> 0 at phase {offset}");
        assert_no_click(&label, &input, &out, 0.45);
    }
}

/// The live front door: an invention with the filter, cutoff writes through
/// `RenderEngine::set_control` while it plays. Writes land on 64-frame
/// block boundaries, chosen so that each lands where the 1 kHz sine is at
/// +/-0.87 of its peak (phase 16 and 32 of 48).
#[test]
fn engine_cutoff_write_does_not_click() {
    let mut engine = engine(chain_doc(1000.0, 20.0));
    let (dry0, wet0) = render(&mut engine, SR as usize + 64);
    set(&engine, "f", "cutoff", 20000.0);
    let (dry1, wet1) = render(&mut engine, 24_064);
    set(&engine, "f", "cutoff", 20.0);
    let (dry2, wet2) = render(&mut engine, SR as usize / 2);
    let dry: Vec<f32> = [dry0, dry1, dry2].concat();
    let wet: Vec<f32> = [wet0, wet1, wet2].concat();
    assert_no_click("engine open/close", &dry, &wet, 0.4);
}

// ---------------------------------------------------------------- criterion 2

/// After a live write, the output has arrived at the new setting by about
/// 10 ms: from 10 ms to 30 ms after it, the output matches a filter that ran
/// at the new setting all along to within 0.02 (4% of the sine's amplitude).
fn assert_arrives(label: &str, start: Value, key: &'static str, value: f32, target: Value) {
    let input = test_sine(SR as usize / 2);
    let at = SR as usize / 2 + 17;
    let out = filter(start).run(&input, &[(at, key, value)]);
    let reference = filter(target).run(&input, &[]);
    let (from, to) = (at + 10 * MS, at + 30 * MS);
    let diff = max_diff(&out[from..to], &reference[from..to]);
    assert!(
        diff <= 0.02,
        "{label}: 10-30 ms after the write the output differs from the target by {diff}"
    );
    // Before the write, the two differ audibly (the test is not vacuous).
    let before = max_diff(&out[at - 480..at], &reference[at - 480..at]);
    assert!(
        before > 0.05,
        "{label}: the write changes the sound ({before})"
    );
}

#[test]
fn opening_cutoff_write_arrives_within_10_ms() {
    assert_arrives(
        "cutoff 20 -> 5000",
        json!({ "filter_type": "lowpass", "cutoff": 20.0 }),
        "cutoff",
        5000.0,
        json!({ "filter_type": "lowpass", "cutoff": 5000.0 }),
    );
}

#[test]
fn closing_cutoff_write_arrives_within_10_ms() {
    assert_arrives(
        "cutoff 20000 -> 300",
        json!({ "filter_type": "lowpass", "cutoff": 20000.0 }),
        "cutoff",
        300.0,
        json!({ "filter_type": "lowpass", "cutoff": 300.0 }),
    );
}

/// The same through the live front door: 10 ms after the write, the level
/// heard matches the steady level at the new cutoff.
#[test]
fn engine_cutoff_write_arrives_within_10_ms() {
    let mut engine = engine(chain_doc(1000.0, 20.0));
    render(&mut engine, SR as usize / 2);
    set(&engine, "f", "cutoff", 5000.0);
    let (_, wet) = render(&mut engine, 30 * MS);
    let mut steady = self::engine(chain_doc(1000.0, 5000.0));
    let (_, want) = render(&mut steady, SR as usize / 2);
    let (got, want) = (peak(&wet[10 * MS..]), peak(&want[want.len() - 20 * MS..]));
    assert!(
        (got - want).abs() <= 0.02 * want,
        "10-30 ms after the write the level is {got}, steady level at 5 kHz is {want}"
    );
}

// ---------------------------------------------------------------- criterion 3

/// Same through the engine: a write before the first render.
#[test]
fn engine_write_before_first_render_lands_at_once() {
    let mut written = engine(chain_doc(1000.0, 20.0));
    set(&written, "f", "cutoff", 5000.0);
    let (_, got) = render(&mut written, SR as usize / 4);
    let mut built = engine(chain_doc(1000.0, 5000.0));
    let (_, want) = render(&mut built, SR as usize / 4);
    assert!(peak(&want) > 0.1, "the reference is audible");
    assert!(
        got == want,
        "a write before the first render does not land at once: max diff {}",
        max_diff(&got, &want)
    );
}

// ---------------------------------------------------------------- criterion 4

/// Never-written filters render bit-exactly as on the previous main.
/// Hashes are FNV-1a of the output's f32 bits, captured on that commit.
#[test]
fn unwritten_filters_render_as_before() {
    let input = noise(SR as usize / 2, 0.5, 3);
    let cases: [(&str, Value, u64); 5] = [
        (
            "lowpass 800 / 0.7",
            json!({ "filter_type": "lowpass", "cutoff": 800.0, "resonance": 0.7 }),
            REF_LOWPASS,
        ),
        (
            "highpass 2500 / 0.3",
            json!({ "filter_type": "highpass", "cutoff": 2500.0, "resonance": 0.3 }),
            REF_HIGHPASS,
        ),
        (
            "bandpass 1200 / 0.9",
            json!({ "filter_type": "bandpass", "cutoff": 1200.0, "resonance": 0.9 }),
            REF_BANDPASS,
        ),
        (
            "lowpass 20000 / 0",
            json!({ "filter_type": "lowpass", "cutoff": 20000.0, "resonance": 0.0 }),
            REF_OPEN,
        ),
        ("defaults", json!({}), REF_DEFAULTS),
    ];
    let mut failures = Vec::new();
    for (label, config, want) in cases {
        let got = hash(&filter(config).run(&input, &[]));
        if got != want {
            failures.push(format!("{label}: {got:#018x} != {want:#018x}"));
        }
    }
    // An audio-rate cutoff input sweep (never a control write).
    let sweep: Vec<f32> = (0..input.len())
        .map(|n| 200.0 * (1.0 + 99.0 * (n as f32 / input.len() as f32)))
        .collect();
    let mut f = filter(json!({ "filter_type": "lowpass", "resonance": 0.5 }));
    let got = hash(&f.run_with(&input, &[], Some(&sweep)));
    if got != REF_SWEEP {
        failures.push(format!(
            "cutoff input sweep: {got:#018x} != {REF_SWEEP:#018x}"
        ));
    }
    assert!(
        failures.is_empty(),
        "renders changed:\n{}",
        failures.join("\n")
    );
}

const REF_LOWPASS: u64 = 0xcceba42bc6b542c2;
const REF_HIGHPASS: u64 = 0xf2f31bb4028ec4dc;
const REF_BANDPASS: u64 = 0xb1585a71e86af1f8;
const REF_OPEN: u64 = 0x0eac78cd0edae9d7;
const REF_DEFAULTS: u64 = 0x235b54ae4a43ec15;
const REF_SWEEP: u64 = 0x009604a71d9202ee;

// ---------------------------------------------------------------- engine rig

fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// A sine oscillator at `freq` dry to the left output and through lowpass
/// `f` at `cutoff` to the right.
fn chain_doc(freq: f64, cutoff: f64) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "src", "type": "oscillator", "config": { "waveform": "sine", "frequency": freq } },
            { "id": "f", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": cutoff, "resonance": 0.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("src", "audio", "dac", "audio_left"),
            conn("src", "audio", "f", "audio"),
            conn("f", "audio", "dac", "audio_right")
        ]
    })
}

fn engine(doc: Value) -> RenderEngine {
    let mut engine = RenderEngine::new(SR);
    (engine.load_json(&doc.to_string())).unwrap_or_else(|err| panic!("invention loads: {err}"));
    engine
}

/// Renders `frames` (rounded down to the engine's 64-frame block).
fn render(engine: &mut RenderEngine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let frames = frames / 64 * 64;
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).expect("renders");
    let left = out.iter().step_by(2).copied().collect();
    let right = out.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

fn set(engine: &RenderEngine, module: &str, key: &str, value: f32) {
    (engine.set_control(module, key, ControlValue::from(value)))
        .unwrap_or_else(|err| panic!("{module}.{key} accepts {value}: {err}"));
}
