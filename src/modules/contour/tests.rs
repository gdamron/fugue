//! Module-level tests: smoothing of control writes and crossfades.

use super::*;

const SR: u32 = 48_000;

fn contour(config: serde_json::Value) -> Box<dyn Module> {
    let built = ContourFactory.build(SR, &config).expect("contour builds");
    match built.module {
        GraphModule::Module(module) => module,
        _ => panic!("contour is a plain module"),
    }
}

fn run(module: &mut dyn Module, frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut left = frames;
    while left > 0 {
        let n = left.min(block);
        module.process(n);
        out.extend_from_slice(&module.output_block(0)[..n]);
        left -= n;
    }
    out
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// A sweep of `shift` writes, one per 64-frame block, glides: the level of
/// a slow envelope output moves smoothly instead of in a staircase.
#[test]
fn a_sweep_of_shift_writes_glides_instead_of_stepping() {
    let mut m = contour(serde_json::json!({
        "output_mode": "envelope", "frequency": 0.01, "shift": 0.0,
    }));
    let warm = run(m.as_mut(), 4800, 64);
    let level = warm.last().copied().unwrap().abs();
    assert!(level > 0.3, "the envelope output carries a level, {level}");
    let mut swept = Vec::new();
    for step in 1..=20 {
        m.set_control("shift", step as f32 / 20.0).unwrap();
        swept.extend(run(m.as_mut(), 64, 64));
    }
    swept.extend(run(m.as_mut(), 960, 64));
    // Each write moves the output by about level / 10; gliding spreads it.
    let worst = max_step(&swept);
    assert!(worst < level * 0.02, "step {worst} at level {level}");
}

/// A mode change crossfades: no output step larger than the cycle's own.
#[test]
fn a_mode_change_crossfades() {
    let mut m = contour(serde_json::json!({ "frequency": 200.0, "range": "audio" }));
    let steady = run(m.as_mut(), 4800, 128);
    let bound = max_step(&steady[480..]);
    m.set_control("mode", 0.0).unwrap();
    let after = run(m.as_mut(), 4800, 128);
    let worst = max_step(&[&steady[steady.len() - 1..], &after[..]].concat());
    assert!(worst <= bound * 1.5, "step {worst}, steady {bound}");
}

/// Renders output 0 with `trigger` fed from `gate(frame)`.
fn run_triggered(module: &mut dyn Module, frames: usize, gate: impl Fn(usize) -> bool) -> Vec<f32> {
    module.set_input_connected(TRIGGER_IN, true);
    let mut out = Vec::with_capacity(frames);
    for start in (0..frames).step_by(64) {
        let block = module.input_block_mut(TRIGGER_IN);
        for (k, slot) in block[..64].iter_mut().enumerate() {
            *slot = f32::from(gate(start + k));
        }
        module.process(64);
        out.extend_from_slice(&module.output_block(0)[..64]);
    }
    out
}

/// A retrigger mid-attack (AD) or a new gate mid-release (AR) carries on
/// from the level the envelope had reached.
#[test]
fn retriggers_carry_on_from_the_current_level() {
    for (mode, gate) in [
        ("ad", (|i| i % 3000 < 10) as fn(usize) -> bool),
        ("ar", |i| i % 30_000 < 12_000 || (19_000..19_500).contains(&(i % 30_000))),
    ] {
        let mut m = contour(serde_json::json!({
            "mode": mode, "output_mode": "envelope", "shift": 1.0, "frequency": 2.0,
        }));
        let out = run_triggered(m.as_mut(), 60_000, gate);
        let peak = out.iter().fold(0.0f32, |p, v| p.max(*v));
        assert!(peak > 0.3, "{mode}: the envelope moves, peak {peak}");
        let worst = max_step(&out);
        assert!(worst < 0.01, "{mode}: step {worst}");
    }
}
