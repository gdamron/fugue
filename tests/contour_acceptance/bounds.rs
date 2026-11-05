//! Criterion 3: outputs stay within +-1 (and 0..1 where unipolar, 0 or 1 for
//! gates), with no NaN, under extreme settings and full random modulation of
//! every control and input.
//!
//! Every declared control is rewritten each 1000 frames to its minimum, its
//! maximum or a random value (numbers capped at +-1e5), choices included;
//! every input carries random signals (gates of random lengths, steps of
//! random frequency, noise at 1x/10x or 1000x depth), connected at random.

use super::rig::*;
use super::*;

const CHUNK: usize = 1000;
const CHUNKS: usize = 60;

fn randomise(c: &mut Contour, rng: &mut Rng, keep: &[&str]) {
    for meta in c.controls.clone() {
        if keep.contains(&meta.key.as_str()) {
            continue;
        }
        match &meta.kind {
            ControlKind::Number { min, max, .. } => {
                let (lo, hi) = (min.max(-1e5), max.min(1e5));
                let v = match rng.below(3) {
                    0 => lo,
                    1 => hi,
                    _ => lo + (hi - lo) * rng.f32(),
                };
                c.set(&meta.key, v);
            }
            ControlKind::String { options: Some(o) } => c.set(&meta.key, rng.below(o.len()) as f32),
            _ => {}
        }
    }
}

fn random_gate(rng: &mut Rng, len: usize, longest: usize) -> Vec<f32> {
    let mut x = Vec::with_capacity(len);
    let mut high = false;
    while x.len() < len {
        let run = 1 + rng.below(longest);
        x.extend(std::iter::repeat(f32::from(high)).take(run));
        high = !high;
    }
    x.truncate(len);
    x
}

fn steps(rng: &mut Rng, len: usize, top: f32) -> Vec<f32> {
    let mut x = Vec::with_capacity(len);
    while x.len() < len {
        let v = top * rng.f32();
        x.extend(std::iter::repeat(v).take(1 + rng.below(4000)));
    }
    x.truncate(len);
    x
}

fn noise(rng: &mut Rng, len: usize, depth: f32) -> Vec<f32> {
    (0..len).map(|_| depth * (2.0 * rng.f32() - 1.0)).collect()
}

/// Random signals for the inputs picked by `mask`.
fn signals(rng: &mut Rng, len: usize) -> Vec<(&'static str, Vec<f32>)> {
    let depth = [1.0, 10.0, 1000.0][rng.below(3)];
    let longest = [4, 60, 800][rng.below(3)];
    let all = vec![
        ("trigger", random_gate(rng, len, 3000)),
        ("clock", random_gate(rng, len, longest)),
        ("frequency", steps(rng, len, 20_000.0)),
        ("frequency_mod", noise(rng, len, depth)),
        ("shape_mod", noise(rng, len, depth.min(10.0))),
        ("slope_mod", noise(rng, len, depth.min(10.0))),
        ("smoothness_mod", noise(rng, len, depth.min(10.0))),
    ];
    let mask = rng.next();
    all.into_iter()
        .enumerate()
        .filter(|(i, _)| mask >> i & 1 == 1 || rng.below(3) > 0)
        .map(|(_, s)| s)
        .collect()
}

fn run(seed: u64, keep: &[&str], config: Value, each: &mut dyn FnMut(&Out)) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
    let mut c = contour(config);
    c.block = [32, 100, 256, 1024][seed as usize % 4];
    let signals = signals(&mut rng, CHUNK * CHUNKS);
    let inputs: Vec<(&str, &[f32])> = signals.iter().map(|(n, s)| (*n, &s[..])).collect();
    for _ in 0..CHUNKS {
        randomise(&mut c, &mut rng, keep);
        if keep.contains(&"mode") {
            c.set("shift", 0.5 + 0.5 * rng.f32());
        }
        each(&c.render(CHUNK, &inputs));
    }
}

#[test]
fn outputs_are_finite_and_within_unity_under_random_modulation() {
    for seed in 0..16 {
        run(seed, &[], json!({}), &mut |out| {
            for (k, x) in out.iter().enumerate() {
                for v in x {
                    assert!(
                        v.is_finite() && v.abs() <= 1.0,
                        "seed {seed}: contour.{k} = {v}"
                    );
                }
            }
        });
    }
}

#[test]
fn ad_and_ar_outputs_are_unipolar_and_envelope_gates_are_binary() {
    for seed in 0..16 {
        let mode = ["ad", "ar"][seed as usize % 2];
        let config = json!({ "mode": mode, "range": "control", "output_mode": "envelope" });
        let keep = ["mode", "range", "output_mode"];
        run(seed, &keep, config, &mut |out| {
            for k in [0, 1] {
                for v in &out[k] {
                    assert!(
                        (0.0..=1.0).contains(v),
                        "seed {seed} {mode}: contour.{k} = {v}"
                    );
                }
            }
            for k in [2, 3] {
                for v in &out[k] {
                    assert!(
                        *v == 0.0 || *v == 1.0,
                        "seed {seed} {mode}: gate contour.{k} = {v}"
                    );
                }
            }
        });
    }
}
