//! Acceptance tests for the `lfo` additions (`division`, `bpm` control and
//! input, `phase`), written from the interface spec alone. They drive only
//! the public API (`ModuleRegistry`, `Module`, `RenderEngine`) and measure
//! what is heard: the cycle length from zero crossings, the waveform shifted
//! by a phase offset, continuity through tempo changes, and that the
//! defaults leave the existing behaviour alone.
//!
//! Spec summary: `division` (default `off`) times one LFO cycle at the tempo
//! and then ignores `rate` (a quarter note at 120 bpm is exactly 2 Hz; a
//! dotted 8th at 100 bpm is 100/60/0.75 Hz); the `bpm` control (default 120)
//! is overridden by a patched `bpm` input above 0; `phase` (cycles, default
//! 0) shifts the waveform and glides over about 20 ms; a tempo, division or
//! rate change changes speed only, so the output is continuous.
//!
//! Assumptions beyond the spec are marked "Assumption" on the test that makes
//! them. Run with `cargo test --release --test lfo_sync_acceptance`.

use fugue::{ControlValue, GraphModule, Module, ModuleRegistry, RenderEngine};
use serde_json::{json, Value};

const SR: u32 = 48_000;
const BLOCK: usize = 128;

fn secs(s: f64) -> usize {
    (s * f64::from(SR)).round() as usize
}

fn lfo(config: Value) -> GraphModule {
    let built = ModuleRegistry::default().build("lfo", SR, &config);
    built
        .unwrap_or_else(|err| panic!("lfo builds with {config}: {err}"))
        .module
}

/// The `bipolar` output of an lfo rendered for `frames`. `bpm_in` patches the
/// `bpm` input with a value per block start; `hook(frame, lfo)` runs before
/// each block.
fn render_with(
    m: &mut GraphModule,
    frames: usize,
    bpm_in: Option<&dyn Fn(usize) -> f32>,
    mut hook: impl FnMut(usize, &mut dyn Module),
) -> Vec<f32> {
    let out_port = m
        .module()
        .output_port_index("bipolar")
        .expect("bipolar output");
    let bpm_port = bpm_in.map(|_| m.module().input_port_index("bpm").expect("bpm input"));
    if let Some(port) = bpm_port {
        m.module_mut().set_input_connected(port, true);
    }
    let mut out = Vec::with_capacity(frames);
    let mut t = 0;
    while t < frames {
        let n = BLOCK.min(frames - t);
        hook(t, m.module_mut());
        if let (Some(port), Some(f)) = (bpm_port, bpm_in) {
            m.module_mut().input_block_mut(port)[..n].fill(f(t));
        }
        assert!(m.module_mut().process(n), "lfo stays active");
        out.extend_from_slice(&m.module().output_block(out_port)[..n]);
        t += n;
    }
    out
}

fn render(config: Value, frames: usize) -> Vec<f32> {
    render_with(&mut lfo(config), frames, None, |_, _| {})
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

/// Fractional positions where `x` crosses zero upward.
fn rising_zero_crossings(x: &[f32]) -> Vec<f64> {
    let mut found = Vec::new();
    for n in 1..x.len() {
        if x[n - 1] < 0.0 && x[n] >= 0.0 {
            found.push((n - 1) as f64 + f64::from(-x[n - 1]) / f64::from(x[n] - x[n - 1]));
        }
    }
    found
}

/// Asserts the sine in `x` has cycles of `period` samples (within one
/// sample), at least `cycles` of them.
fn assert_period(x: &[f32], period: f64, cycles: usize, what: &str) {
    assert_period_within(x, period, 1.0, cycles, what);
}

/// As `assert_period`, with the cycle length allowed to be off by `tol`
/// samples.
fn assert_period_within(x: &[f32], period: f64, tol: f64, cycles: usize, what: &str) {
    let crossings = rising_zero_crossings(x);
    assert!(
        crossings.len() > cycles,
        "{what}: only {} cycles",
        crossings.len()
    );
    for w in crossings.windows(2) {
        assert!(
            (w[1] - w[0] - period).abs() <= tol,
            "{what}: cycle of {} samples, want {period}",
            w[1] - w[0]
        );
    }
}

fn sine_config(extra: Value) -> Value {
    let mut config = json!({ "waveform": "sine" });
    for (k, v) in extra.as_object().unwrap() {
        config[k] = v.clone();
    }
    config
}

#[test]
fn a_quarter_note_at_120_bpm_is_exactly_2_hz_and_ignores_rate() {
    for rate in [0.5, 2.0, 9.0] {
        let config = sine_config(json!({ "division": "quarter_note", "bpm": 120.0, "rate": rate }));
        assert_period(&render(config, secs(5.0)), 24_000.0, 3, "quarter note");
    }
}

#[test]
fn a_dotted_eighth_at_100_bpm_runs_at_100_over_60_over_0_75_hz() {
    let config = sine_config(json!({ "division": "dotted_8th_note", "bpm": 100.0 }));
    assert_period(&render(config, secs(5.0)), 21_600.0, 3, "dotted 8th");
}

/// Other note values: whole at 120 is 2 s, a 16th triplet at 90 is 1/6 beat.
#[test]
fn other_note_values_set_the_cycle_length() {
    for (division, bpm, period) in [
        ("whole_note", 120.0, 96_000.0),
        ("16th_triplet", 90.0, 5_333.333),
        ("half_note", 60.0, 96_000.0),
    ] {
        let config = sine_config(json!({ "division": division, "bpm": bpm }));
        assert_period(&render(config, secs(8.0)), period, 2, division);
    }
}

/// The fraction spellings are the same note values.
#[test]
fn fraction_spellings_match_the_named_note_values() {
    for (fraction, named) in [
        ("1/4", "quarter_note"),
        ("1/8.", "dotted_8th_note"),
        ("1/8t", "8th_triplet"),
    ] {
        let a = render(
            sine_config(json!({ "division": fraction, "bpm": 100.0 })),
            secs(2.0),
        );
        let b = render(
            sine_config(json!({ "division": named, "bpm": 100.0 })),
            secs(2.0),
        );
        assert!(peak(&a) > 0.5, "{fraction} oscillates");
        assert_eq!(a, b, "{fraction} is {named}");
    }
}

#[test]
fn the_bpm_control_sets_the_tempo() {
    let config = sine_config(json!({ "division": "quarter_note", "bpm": 60.0 }));
    assert_period(&render(config, secs(6.0)), 48_000.0, 2, "60 bpm");
}

/// A patched `bpm` input above 0 overrides the control.
#[test]
fn the_bpm_input_overrides_the_bpm_control() {
    let config = sine_config(json!({ "division": "quarter_note", "bpm": 60.0 }));
    let tempo = |_: usize| 240.0;
    let x = render_with(&mut lfo(config), secs(3.0), Some(&tempo), |_, _| {});
    assert_period(&x[BLOCK..], 12_000.0, 8, "240 bpm input");
}

/// Assumption: the input applies only while finite and above 0, so a patched
/// 0, negative or NaN tempo leaves the `bpm` control in charge.
#[test]
fn an_unusable_bpm_input_falls_back_to_the_control() {
    for bad in [0.0f32, -50.0, f32::NAN, f32::INFINITY] {
        let config = sine_config(json!({ "division": "quarter_note", "bpm": 120.0 }));
        let tempo = move |_: usize| bad;
        let x = render_with(&mut lfo(config), secs(3.0), Some(&tempo), |_, _| {});
        assert_period(&x[BLOCK..], 24_000.0, 2, &format!("bpm input {bad}"));
    }
}

/// With `division` off the LFO runs at `rate` and the tempo does nothing.
/// The free-running phase accumulates in f32 and drifts a few samples over
/// seconds, so a cycle is held to 0.1% of 1/rate rather than to one sample.
#[test]
fn division_off_runs_at_rate_whatever_the_tempo() {
    let tol = 16_000.0 * 0.001;
    let config = sine_config(json!({ "rate": 3.0, "bpm": 400.0 }));
    assert_period_within(&render(config, secs(3.0)), 16_000.0, tol, 3, "3 Hz");
    let tempo = |_: usize| 90.0;
    let x = render_with(
        &mut lfo(sine_config(json!({ "rate": 3.0 }))),
        secs(3.0),
        Some(&tempo),
        |_, _| {},
    );
    assert_period_within(&x[BLOCK..], 16_000.0, tol, 3, "3 Hz with a bpm input");
}

/// Two LFOs built alike, one with `phase` p, give the same waveform shifted
/// by p cycles: `shifted[n] == plain[n + p * period]`.
#[test]
fn a_phase_offset_shifts_the_waveform_by_that_many_cycles() {
    let base = render(sine_config(json!({ "rate": 2.0 })), secs(3.0));
    for (phase, shift) in [(0.25, 6_000), (0.5, 12_000), (0.1, 2_400), (0.75, 18_000)] {
        let moved = render(
            sine_config(json!({ "rate": 2.0, "phase": phase })),
            secs(3.0),
        );
        // Past the 20 ms glide-in a start value might have.
        let worst = (secs(0.25)..secs(2.0))
            .map(|n| (moved[n] - base[n + shift]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 2e-3, "phase {phase}: worst difference {worst}");
    }
}

/// Assumption: a `phase` given in config is the starting value (no glide
/// from 0 at build): a sine with phase 0.25 starts at its peak.
#[test]
fn a_sine_with_a_quarter_cycle_of_phase_starts_at_its_peak() {
    let x = render(
        sine_config(json!({ "rate": 2.0, "phase": 0.25 })),
        secs(1.0),
    );
    assert!(
        x[0] >= 0.99 * peak(&x),
        "starts at {} of peak {}",
        x[0],
        peak(&x)
    );
}

/// A live `phase` change glides, so the output never steps, and settles on
/// the shifted waveform.
#[test]
fn a_phase_change_glides_without_a_step_and_settles_on_the_shift() {
    let base = render(sine_config(json!({ "rate": 2.0 })), secs(3.0));
    let mut m = lfo(sine_config(json!({ "rate": 2.0 })));
    let x = render_with(&mut m, secs(3.0), None, |t, module| {
        if t >= secs(1.0) && t < secs(1.0) + BLOCK {
            module.set_control("phase", 0.5).expect("phase accepts 0.5");
        }
    });
    // A half-cycle in 20 ms is at most ~28 Hz, 2*pi*28/48000 = 3.7e-3 a step.
    let top = peak(&base);
    assert!(max_step(&x) < 0.01 * top, "largest step {}", max_step(&x));
    for n in secs(2.0)..secs(3.0) {
        assert!(
            (x[n] + base[n]).abs() < 0.01 * top,
            "frame {n}: {} vs {}",
            x[n],
            -base[n]
        );
    }
}

/// A tempo change through the `bpm` control changes the speed only.
#[test]
fn a_bpm_control_change_keeps_the_output_continuous() {
    let config = sine_config(json!({ "division": "quarter_note", "bpm": 60.0 }));
    let mut m = lfo(config);
    let x = render_with(&mut m, secs(4.0), None, |t, module| {
        for (at, bpm) in [(secs(1.0), 240.0), (secs(2.0), 30.0), (secs(3.0), 1000.0)] {
            if t >= at && t < at + BLOCK {
                module
                    .set_control("bpm", bpm)
                    .expect("bpm accepts the write");
            }
        }
    });
    // Fastest is 1000 bpm = 16.7 Hz: 2*pi*16.7/48000 = 2.2e-3 a step.
    let top = peak(&x);
    assert!(
        max_step(&x) <= 1.05 * top * 2.2e-3,
        "largest step {}",
        max_step(&x)
    );
    assert_period(&x[secs(1.0) + 2 * BLOCK..secs(2.0)], 12_000.0, 2, "240 bpm");
}

/// A tempo change through the `bpm` input changes the speed only, and the new
/// tempo takes effect.
#[test]
fn a_bpm_input_change_keeps_the_output_continuous() {
    let config = sine_config(json!({ "division": "quarter_note" }));
    let tempo = |t: usize| match t / secs(2.0) {
        0 => 120.0,
        1 => 240.0,
        _ => 90.0,
    };
    let x = render_with(&mut lfo(config), secs(8.0), Some(&tempo), |_, _| {});
    let top = peak(&x);
    assert!(
        max_step(&x) <= 1.05 * top * 2.0 * std::f32::consts::PI * 4.0 / SR as f32,
        "largest step {}",
        max_step(&x)
    );
    // Cycles wholly inside a tempo segment have that segment's length.
    let crossings = rising_zero_crossings(&x);
    for (from, to, period) in [
        (0, secs(2.0), 24_000.0),
        (secs(2.0), secs(4.0), 12_000.0),
        (secs(4.0), secs(8.0), 32_000.0),
    ] {
        let inside: Vec<f64> = crossings
            .iter()
            .copied()
            .filter(|&c| c > (from + 2 * BLOCK) as f64 && c < to as f64)
            .collect();
        assert!(inside.len() >= 2, "segment {from}..{to} has cycles");
        for w in inside.windows(2) {
            assert!(
                (w[1] - w[0] - period).abs() <= 1.0,
                "segment {from}: cycle {} want {period}",
                w[1] - w[0]
            );
        }
    }
}

fn dac_doc(lfo_config: Value) -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "wave", "type": "lfo", "config": lfo_config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "wave", "from_port": "bipolar", "to": "dac", "to_port": "audio_left" }
        ]
    })
    .to_string()
}

/// A division (a choice) written live changes the speed only. Rendered in an
/// engine, where choices are written as strings.
#[test]
fn a_live_division_change_keeps_the_output_continuous() {
    let mut engine = RenderEngine::new(SR);
    let config = sine_config(json!({ "division": "half_note", "bpm": 120.0 }));
    engine.load_json(&dac_doc(config)).expect("invention loads");
    let mut x = Vec::new();
    for (division, frames) in [
        ("half_note", 96_000),
        ("16th_note", 48_000),
        ("quarter_note", 48_000),
    ] {
        engine
            .set_control("wave", "division", ControlValue::from(division))
            .expect("division accepts the note value");
        let mut buf = vec![0.0f32; frames * 2];
        engine.render_interleaved(&mut buf).expect("renders");
        x.extend(buf.iter().step_by(2));
    }
    // Fastest is a 16th at 120 bpm = 8 Hz: 2*pi*8/48000 = 1.05e-3 a step.
    let top = peak(&x);
    assert!(
        max_step(&x) <= 1.05 * top * 1.05e-3,
        "largest step {}",
        max_step(&x)
    );
    let tail = &x[96_000 + 6_000..96_000 + 42_000];
    assert_period(tail, 6_000.0, 4, "16th note at 120 bpm");
}

/// Existing behaviour is unchanged at the defaults: spelling the defaults out
/// gives the same samples, and the waveform matches what the plain lfo
/// produced before the additions (samples read from the plain lfo on main).
#[test]
fn the_new_controls_at_their_defaults_leave_the_lfo_unchanged() {
    for waveform in ["sine", "triangle", "square"] {
        let plain = json!({ "rate": 3.0, "waveform": waveform });
        let spelled = json!({ "rate": 3.0, "waveform": waveform, "division": "off", "phase": 0.0, "bpm": 120.0 });
        let (a, b) = (render(plain, secs(2.0)), render(spelled, secs(2.0)));
        assert_eq!(a, b, "{waveform}");
    }
    let sine = render(json!({ "rate": 3.0, "waveform": "sine" }), secs(2.0));
    for (n, want) in GOLDEN_SINE {
        assert!(
            (sine[n] - want).abs() < 1e-5,
            "sample {n}: {} vs {want}",
            sine[n]
        );
    }
}

/// Samples of the 3 Hz sine lfo, captured from the lfo before the additions.
const GOLDEN_SINE: [(usize, f32); 4] = [
    (1, 0.000_392_699),
    (1_000, 0.382_680),
    (12_345, -0.990_764),
    (95_999, 0.006_289_1),
];
