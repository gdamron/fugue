//! Criterion "the delay atom declares its minimum delay and clamps time to
//! at least that", and the emit/absorb contract of the hook.

use super::*;
use fugue::FeedbackCut;

fn cut_of(config: Value) -> Option<FeedbackCut> {
    built_delay(config).module().feedback_cut()
}

fn declares(frames: usize) -> Option<FeedbackCut> {
    // Port 0 is `audio`; the cut absorbs that input only.
    Some(FeedbackCut {
        frames,
        absorbed: &[0],
    })
}

#[test]
fn the_audio_input_is_port_zero() {
    assert_eq!(
        built_delay(json!({})).module().input_port_index("audio"),
        Some(0)
    );
}

/// Default `min_time` is the one-sample floor.
#[test]
fn a_delay_without_min_time_declares_one_frame() {
    assert_eq!(cut_of(json!({})), declares(1));
    assert_eq!(cut_of(json!({ "time": 0.5, "feedback": 0.3 })), declares(1));
}

/// `min_time` x sample rate, rounded up to a whole sample (a value within
/// 1e-3 of a whole sample counts as that sample); clamped to `[0, max_time]`.
#[test]
fn min_time_sets_the_declared_frames() {
    let sr = f64::from(SR);
    let cases: [(f64, usize); 7] = [
        (0.01, 480),
        (0.5 / sr, 1),
        (480.0004 / sr, 480),
        (480.5 / sr, 481),
        (480.01 / sr, 481),
        (-0.1, 1),
        (0.0, 1),
    ];
    for (min_time, frames) in cases {
        let got = cut_of(json!({ "min_time": min_time }));
        assert_eq!(got, declares(frames), "min_time {min_time}");
    }
    // Past max_time the clamp wins.
    let long = cut_of(json!({ "max_time": 0.5, "min_time": 2.0 }));
    assert_eq!(long, declares(24_000));
}

/// A module that guarantees no delay declares none.
#[test]
fn modules_without_a_guaranteed_delay_declare_none() {
    let built = ModuleRegistry::default()
        .build("vca", SR, &json!({}))
        .unwrap();
    assert_eq!(built.module.module().feedback_cut(), None);
}

/// Runs `m` over `input` with the `time_mod` input held at `mod_in`.
fn process(m: &mut GraphModule, input: &[f32], mod_in: f32) -> Vec<f32> {
    let m = m.module_mut();
    let (audio, time_mod) = (
        m.input_port_index("audio").unwrap(),
        m.input_port_index("time_mod").unwrap(),
    );
    m.set_input_connected(audio, true);
    let mut out = Vec::new();
    for chunk in input.chunks(128) {
        let n = chunk.len();
        m.input_block_mut(audio)[..n].copy_from_slice(chunk);
        m.input_block_mut(time_mod)[..n].fill(mod_in);
        assert!(m.process(n));
        out.extend_from_slice(&m.output_block(0)[..n]);
    }
    out
}

fn impulse(len: usize, at: usize) -> Vec<f32> {
    let mut x = vec![0.0; len];
    x[at] = 0.5;
    x
}

fn hits(x: &[f32]) -> Vec<usize> {
    x.iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > 1e-4)
        .map(|(i, _)| i)
        .collect()
}

/// `time` clamps to at least `min_time` and reads back clamped (from config
/// and from the control), and at most `max_time`.
#[test]
fn time_reads_back_clamped_to_min_time() {
    let cfg = json!({ "min_time": 0.01, "time": 0.001, "max_time": 1.0 });
    let mut d = built_delay(cfg);
    let read = |d: &GraphModule| d.module().get_control("time").unwrap();
    assert!(
        (read(&d) - 0.01).abs() < 1e-6,
        "config time clamped, read {}",
        read(&d)
    );
    d.module_mut().set_control("time", 0.2).unwrap();
    assert!((read(&d) - 0.2).abs() < 1e-6);
    d.module_mut().set_control("time", 0.0).unwrap();
    assert!(
        (read(&d) - 0.01).abs() < 1e-6,
        "control time clamped, read {}",
        read(&d)
    );
    d.module_mut().set_control("time", 5.0).unwrap();
    assert!(
        (read(&d) - 1.0).abs() < 1e-6,
        "clamped to max_time, read {}",
        read(&d)
    );
}

/// An impulse comes out no earlier than `min_time`, even when `time` asks for
/// less, and even when `time_mod` pulls the time shorter still.
#[test]
fn an_impulse_comes_out_no_earlier_than_min_time() {
    let input = impulse(12_000, 5000);
    // time below min_time.
    let mut d = built_delay(json!({ "min_time": 0.01, "time": 0.001 }));
    assert_eq!(hits(&process(&mut d, &input, 0.0)), [5000 + 480]);
    // time_mod pulling far below min_time (depth 1 s per unit, -1 = -1 s).
    let cfg = json!({ "min_time": 0.01, "time": 0.02, "time_mod_depth": 1.0 });
    let mut d = built_delay(cfg);
    assert_eq!(hits(&process(&mut d, &input, -1.0)), [5000 + 480]);
    // Without a floor the modulated delay may be shorter than min_time would allow.
    let mut d = built_delay(json!({ "time": 0.02, "time_mod_depth": 0.005 }));
    assert_eq!(hits(&process(&mut d, &input, -1.0)), [5000 + 960 - 240]);
}

/// The hook's contract: `emit(n)` then `absorb(n)` over any split of a
/// stretch into pieces of at most `cut.frames` gives the output `process`
/// gives. `emit` must not read the absorbed input (it is NaN then), and
/// internal feedback and a fractional `time_mod` read are included.
#[test]
fn emit_then_absorb_equals_process() {
    let cfg = json!({ "min_time": 0.002, "time": 0.01, "feedback": 0.5, "time_mod_depth": 0.001 });
    let input = noise(9000, 0.4, 3);
    let mut reference = built_delay(cfg.clone());
    let want = process(&mut reference, &input, 0.3);

    let mut d = built_delay(cfg);
    let m = d.module_mut();
    let cut = m.feedback_cut().expect("declares a cut");
    assert_eq!(cut.frames, 96);
    let (audio, time_mod) = (
        m.input_port_index("audio").unwrap(),
        m.input_port_index("time_mod").unwrap(),
    );
    m.set_input_connected(audio, true);
    let sizes = [96, 17, 1, 64, 96, 5, 33];
    let (mut got, mut at, mut turn) = (Vec::new(), 0, 0);
    while at < input.len() {
        let n = sizes[turn % sizes.len()].min(input.len() - at);
        turn += 1;
        m.input_block_mut(audio)[..n].fill(f32::NAN);
        m.input_block_mut(time_mod)[..n].fill(0.3);
        m.emit(n);
        got.extend_from_slice(&m.output_block(0)[..n]);
        m.input_block_mut(audio)[..n].copy_from_slice(&input[at..at + n]);
        m.absorb(n);
        at += n;
    }
    assert_bits(&got, &want, "emit/absorb vs process");
}
