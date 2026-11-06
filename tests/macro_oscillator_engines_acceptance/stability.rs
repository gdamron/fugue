//! Bounded, finite output under random controls and random triggers.

use super::rig::*;
use super::*;
use fugue::dsp::Rng;

/// Frames per `process` call, and whether the `frequency`, `frequency_mod`
/// and `level` inputs change every sample (held for the block otherwise).
/// Controls change on every call.
struct Mode {
    block: usize,
    per_sample: bool,
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + rng.unit() * (hi - lo)
}

fn randomize_controls(v: &mut Voice, rng: &mut Rng) {
    for meta in v.controls.clone() {
        if meta.key == "model" {
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

fn check(x: &[f32], name: &str, port: &str, mode: usize, frame: usize) {
    for (i, v) in x.iter().enumerate() {
        assert!(
            v.is_finite(),
            "{name} {port} non-finite ({v}) in mode {mode}, frame {}",
            frame + i
        );
        assert!(
            v.abs() <= 1.0,
            "{name} {port} out of range ({v}) in mode {mode}, frame {}",
            frame + i
        );
    }
}

/// Drives a fresh voice on `name` with random controls and inputs for `secs`
/// seconds, split 40/40/20 between three block sizes.
fn stress(name: &str, seed: u64, secs: f64) {
    let mut v = engine(name);
    let mut rng = Rng::new(seed);
    for port in [v.frequency_in, v.frequency_mod_in, v.trigger_in, v.level_in] {
        v.connect(port);
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
            randomize_controls(&mut v, &mut rng);
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
            check(v.audio(n), name, "audio", index, frame);
            check(v.aux(n), name, "aux", index, frame);
            frame += n;
        }
    }
}

/// Criterion: "audio and aux within +-1, finite" under "random control writes
/// and random triggers for a few seconds": every declared control (except
/// `model`) over its whole range on each call, `frequency` 20 Hz to 20 kHz,
/// `frequency_mod` in [-1, 1], `level` in [0, 1] and random-length trigger
/// pulses; per 1024-frame block, per 32-frame block with per-sample inputs,
/// and per sample; 3 s per engine, fixed seed.
mod random_modulation_stays_bounded {
    use super::*;

    fn run(name: &str) {
        let position = NEW.iter().position(|n| *n == name).unwrap();
        stress(name, 0xE16 + position as u64, 3.0);
    }

    each_new_model!(run);
}

/// Criterion: same bound at the extremes of every control and input in turn
/// (all controls at min, then at max, at the ends of the audible range, with
/// full-scale modulation and a trigger train every 0.25 s).
mod extremes_stay_bounded {
    use super::*;

    fn run(name: &str) {
        for at_max in [false, true] {
            for hz in [20.0f32, 20_000.0] {
                let mut v = engine(name);
                for meta in v.controls.clone() {
                    if meta.key == "model" {
                        continue;
                    }
                    if let ControlKind::Number { min, max, .. } = meta.kind {
                        v.set(&meta.key, if at_max { max } else { min });
                    }
                }
                let (f, m, l) = (v.frequency_in, v.frequency_mod_in, v.level_in);
                v.hold(f, hz);
                v.hold(m, if at_max { 1.0 } else { -1.0 });
                v.hold(l, 1.0);
                let train: Vec<usize> = (0..4).map(|k| T0 + k * seconds(0.25)).collect();
                let (audio, aux) = v.render_driven(seconds(1.0), pulses(&train));
                check(&audio, name, "audio", 0, 0);
                check(&aux, name, "aux", 0, 0);
            }
        }
    }

    each_new_model!(run);
}
