//! Criterion 2: bounded, finite, normal output under random modulation.

use super::rig::*;
use super::*;
use fugue::dsp::Rng;

/// One way of driving the module: frames per `process` call, and whether the
/// `frequency`, `frequency_mod` and `level` inputs change every sample (they
/// are held for the block otherwise). Controls change on every call.
struct Mode {
    block: usize,
    per_sample: bool,
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + rng.unit() * (hi - lo)
}

fn randomize_controls(v: &mut Voice, rng: &mut Rng, skip_model: bool) {
    for meta in v.controls.clone() {
        if skip_model && meta.key == "model" {
            continue;
        }
        let value = match &meta.kind {
            ControlKind::Number { min, max, .. } => uniform(rng, *min, *max),
            ControlKind::Bool => (rng.next_u32() & 1) as f32,
            ControlKind::String { options: Some(o) } => (rng.next_u32() as usize % o.len()) as f32,
            ControlKind::String { options: None } => continue,
        };
        v.set(&meta.key, value);
    }
}

/// Audible frequency, log-uniform over 20 Hz to 20 kHz.
fn audible(rng: &mut Rng) -> f32 {
    20.0 * 1000f32.powf(rng.unit())
}

fn check(x: &[f32], what: &str, mode: usize, frame: usize) {
    for (i, v) in x.iter().enumerate() {
        assert!(
            *v == 0.0 || v.is_normal(),
            "{what} non-finite or subnormal ({v:e}) in mode {mode}, frame {}",
            frame + i
        );
        assert!(v.abs() <= 1.0, "{what} out of range ({v}) in mode {mode}");
    }
}

/// Drives a fresh voice with random controls and inputs for `secs` seconds,
/// split 40/40/20 between the three modes; fixes `model` when given.
fn stress(seed: u64, secs: f64, fixed_model: Option<usize>) {
    let mut v = voice(json!({}));
    let mut rng = Rng::new(seed);
    for port in [v.frequency_in, v.frequency_mod_in, v.trigger_in, v.level_in] {
        v.connect(port);
    }
    if let Some(position) = fixed_model {
        v.select(position);
    }
    let modes = [
        (
            Mode {
                block: 1024,
                per_sample: false,
            },
            0.4,
        ),
        (
            Mode {
                block: 32,
                per_sample: true,
            },
            0.4,
        ),
        (
            Mode {
                block: 1,
                per_sample: true,
            },
            0.2,
        ),
    ];
    let (freq, fm, trig, level) = (v.frequency_in, v.frequency_mod_in, v.trigger_in, v.level_in);
    let mut high_left = 0u32;
    let mut frame = 0;
    for (index, (mode, share)) in modes.iter().enumerate() {
        let calls = (seconds(secs) as f64 * share) as usize / mode.block;
        for _ in 0..calls {
            randomize_controls(&mut v, &mut rng, fixed_model.is_some());
            let n = mode.block;
            let held = (audible(&mut rng), uniform(&mut rng, -1.0, 1.0), rng.unit());
            for i in 0..n {
                let (f, m, l) = if mode.per_sample {
                    (audible(&mut rng), uniform(&mut rng, -1.0, 1.0), rng.unit())
                } else {
                    held
                };
                // Trigger: random-length pulses, so edges land anywhere.
                let t = if high_left > 0 {
                    high_left -= 1;
                    1.0
                } else if rng.unit() < 0.003 {
                    high_left = rng.next_u32() % 64;
                    1.0
                } else {
                    0.0
                };
                v.input_block(freq)[i] = f;
                v.input_block(fm)[i] = m;
                v.input_block(level)[i] = l;
                v.input_block(trig)[i] = t;
            }
            v.process(n);
            check(v.audio(n), "audio", index, frame);
            check(v.aux(n), "aux", index, frame);
            frame += n;
        }
    }
}

/// Criterion: "Under full-range random modulation of every control and input
/// for 10 s, `audio` and `aux` stay within [-1, 1], with no NaN, infinities
/// or subnormals": every declared control over its range (model included, so
/// engines crossfade constantly), `frequency` 20 Hz to 20 kHz, `frequency_mod`
/// in [-1, 1], trigger edges and `level` in [0, 1]; per 1024-frame block,
/// per 32-frame block with per-sample inputs, and per sample; fixed seed.
/// Assumption: `frequency_mod` is meant to span [-1, 1].
#[test]
fn ten_seconds_of_random_modulation_stay_bounded() {
    stress(0xF0661E, 10.0, None);
}

/// Criterion: same bound with the model held on each engine in turn, so every
/// engine meets the full-range modulation rather than a crossfade of two.
#[test]
fn each_model_stays_bounded_under_random_modulation() {
    let count = voice(json!({})).models.len();
    for position in 0..count {
        stress(0xAB5 + position as u64, 4.0, Some(position));
    }
}

/// Criterion: same bound at the extremes of every control and input in
/// turn (all controls at min, then at max, with the frequency input at the
/// ends of the audible range and full-scale modulation).
#[test]
fn extremes_stay_bounded() {
    let count = voice(json!({})).models.len();
    for position in 0..count {
        for at_max in [false, true] {
            for hz in [20.0f32, 20_000.0] {
                let mut v = voice(json!({}));
                v.select(position);
                for meta in v.controls.clone() {
                    if let ControlKind::Number { min, max, .. } = meta.kind {
                        v.set(&meta.key, if at_max { max } else { min });
                    }
                }
                let (f, m, l) = (v.frequency_in, v.frequency_mod_in, v.level_in);
                v.hold(f, hz);
                v.hold(m, if at_max { 1.0 } else { -1.0 });
                v.hold(l, 1.0);
                let x = v.render(seconds(1.0));
                check(&x, "audio", position, 0);
            }
        }
    }
}
