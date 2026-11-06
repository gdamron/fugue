//! A3: bounded, finite output under random modulation and at extremes.

use super::rig::*;
use super::*;
use fugue::dsp::Rng;

/// One way of driving the module: frames per `process` call, and whether the
/// `frequency`, `frequency_mod` and `level` inputs change every sample.
struct Mode {
    block: usize,
    per_sample: bool,
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + rng.unit() * (hi - lo)
}

/// Every control is randomized over its range, except `model`, and except
/// `pinned` keys, which are drawn from the range given for them.
pub fn randomize_controls(v: &mut Voice, rng: &mut Rng, pinned: &[(&str, f32, f32)]) {
    for meta in v.controls.clone() {
        if meta.key == "model" {
            continue;
        }
        let value = match (&meta.kind, pinned.iter().find(|p| p.0 == meta.key)) {
            (_, Some((_, lo, hi))) => uniform(rng, *lo, *hi),
            (ControlKind::Number { min, max, .. }, _) => uniform(rng, *min, *max),
            (ControlKind::Bool, _) => (rng.next_u32() & 1) as f32,
            (ControlKind::String { options: Some(o) }, _) => {
                (rng.next_u32() as usize % o.len()) as f32
            }
            (ControlKind::String { options: None }, _) => continue,
        };
        v.set(&meta.key, value);
    }
}

fn audible(rng: &mut Rng) -> f32 {
    20.0 * 1000f32.powf(rng.unit())
}

pub fn check(x: &[f32], what: &str, context: &str) {
    for (i, v) in x.iter().enumerate() {
        assert!(
            *v == 0.0 || v.is_normal(),
            "{what} non-finite or subnormal ({v:e}) in {context}, frame {i}"
        );
        assert!(v.abs() <= 1.0, "{what} out of range ({v}) in {context}");
    }
}

/// Drives a fresh voice on `model` with random controls and inputs for
/// `secs` seconds, split 40/40/20 between three block modes.
pub fn stress(seed: u64, secs: f64, model: usize, pinned: &[(&str, f32, f32)]) {
    let mut v = voice(json!({}));
    let mut rng = Rng::new(seed);
    let (freq, fm, trig, level) = (v.frequency_in, v.frequency_mod_in, v.trigger_in, v.level_in);
    for port in [freq, fm, trig, level] {
        v.connect(port);
    }
    v.select(model);
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
    let mut high_left = 0u32;
    for (index, (mode, share)) in modes.iter().enumerate() {
        let calls = (seconds(secs) as f64 * share) as usize / mode.block;
        for _ in 0..calls {
            randomize_controls(&mut v, &mut rng, pinned);
            let n = mode.block;
            let held = (audible(&mut rng), uniform(&mut rng, -1.0, 1.0), rng.unit());
            for i in 0..n {
                let (f, m, l) = if mode.per_sample {
                    (audible(&mut rng), uniform(&mut rng, -1.0, 1.0), rng.unit())
                } else {
                    held
                };
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
            let context = format!("mode {index}");
            check(v.audio(n), "audio", &context);
            check(v.aux(n), "aux", &context);
        }
    }
}

/// Criterion A3: "`audio` and `aux` stay finite and within +-1 under random
/// macro and pitch modulation": every control over its range (the three
/// macros included), `frequency` 20 Hz to 20 kHz, `frequency_mod` in [-1, 1],
/// trigger edges and `level` in [0, 1]; per 1024-frame block, per 32-frame
/// block with per-sample inputs, and per sample; fixed seeds.
#[test]
fn wavetable_stays_bounded_under_random_modulation() {
    let position = wavetable();
    for seed in [0x7AB1E, 0xBEEF] {
        stress(seed, 3.0, position, &[]);
    }
}

/// Criterion A3: "... and at extreme settings": every corner of the three
/// macros, with the frequency input at both ends of the audible range and
/// full-scale modulation in both directions.
#[test]
fn wavetable_stays_bounded_at_extreme_settings() {
    let position = wavetable();
    for corner in 0..8u32 {
        let macros = [
            ("harmonics", (corner & 1) as f32),
            ("timbre", ((corner >> 1) & 1) as f32),
            ("morph", ((corner >> 2) & 1) as f32),
        ];
        for hz in [20.0f32, 20_000.0] {
            for fm_value in [-1.0f32, 1.0] {
                let mut v = voice(json!({}));
                v.select(position);
                v.set_all(&macros);
                v.set("frequency_mod_depth", 1000.0);
                let (f, m, l) = (v.frequency_in, v.frequency_mod_in, v.level_in);
                v.hold(f, hz);
                v.hold(m, fm_value);
                v.hold(l, 1.0);
                let (audio, aux) = v.render_both(seconds(0.4));
                let context = format!("{macros:?}, {hz} Hz, fm {fm_value}");
                check(&audio, "audio", &context);
                check(&aux, "aux", &context);
            }
        }
    }
}
