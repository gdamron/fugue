//! C3: bounded, finite output under random modulation, with and without
//! triggers.

use super::rig::*;
use super::*;
use fugue::dsp::Rng;

/// How `trigger` is driven.
#[derive(Clone, Copy, Debug)]
enum Trigger {
    /// Not connected: the voice drones.
    Unpatched,
    /// Random-length pulses (up to 64 frames) at random times.
    Random,
    /// 480 Hz pulses: 10 frames high every 100 frames.
    Fast,
    /// Held high throughout.
    Held,
}

const TRIGGERS: [Trigger; 4] = [
    Trigger::Unpatched,
    Trigger::Random,
    Trigger::Fast,
    Trigger::Held,
];

struct Mode {
    block: usize,
    per_sample: bool,
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + rng.unit() * (hi - lo)
}

/// Every control is randomized over its range except `model`; keys in
/// `pinned` are drawn from the range given for them.
fn randomize_controls(v: &mut Voice, rng: &mut Rng, pinned: &[(&str, f32, f32)]) {
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

fn check(x: &[f32], what: &str, context: &str) {
    for (i, v) in x.iter().enumerate() {
        assert!(
            *v == 0.0 || v.is_normal(),
            "{what} non-finite or subnormal ({v:e}) in {context}, frame {i}"
        );
        assert!(v.abs() <= 1.0, "{what} out of range ({v}) in {context}");
    }
}

/// Drives a fresh `six_op_fm` voice with random controls (`pinned` ones
/// restricted), `frequency` 20 Hz to 20 kHz, `frequency_mod` in [-1, 1] and
/// `level` in [0, 1], for `secs` seconds split 40/40/20 between three block
/// modes, with `trigger` driven as given.
fn stress(seed: u64, secs: f64, trigger: Trigger, pinned: &[(&str, f32, f32)]) {
    let mut v = voice(json!({}));
    let mut rng = Rng::new(seed);
    let (freq, fm, trig, level) = (v.frequency_in, v.frequency_mod_in, v.trigger_in, v.level_in);
    for port in [freq, fm, level] {
        v.connect(port);
    }
    if !matches!(trigger, Trigger::Unpatched) {
        v.connect(trig);
    }
    v.select(six_op());
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
    let (mut high_left, mut frame) = (0u32, 0usize);
    for (index, (mode, share)) in modes.iter().enumerate() {
        let calls = (seconds(secs) as f64 * share) as usize / mode.block;
        for _ in 0..calls {
            randomize_controls(&mut v, &mut rng, pinned);
            let n = mode.block;
            let held = (
                20.0 * 1000f32.powf(rng.unit()),
                uniform(&mut rng, -1.0, 1.0),
                rng.unit(),
            );
            for i in 0..n {
                let (f, m, l) = if mode.per_sample {
                    (
                        20.0 * 1000f32.powf(rng.unit()),
                        uniform(&mut rng, -1.0, 1.0),
                        rng.unit(),
                    )
                } else {
                    held
                };
                let t = match trigger {
                    Trigger::Unpatched => 0.0,
                    Trigger::Held => 1.0,
                    Trigger::Fast => f32::from(((frame + i) % 100) < 10),
                    Trigger::Random => {
                        if high_left > 0 {
                            high_left -= 1;
                            1.0
                        } else if rng.unit() < 0.003 {
                            high_left = rng.next_u32() % 64;
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                v.input_block(freq)[i] = f;
                v.input_block(fm)[i] = m;
                v.input_block(level)[i] = l;
                if !matches!(trigger, Trigger::Unpatched) {
                    v.input_block(trig)[i] = t;
                }
            }
            v.process(n);
            let context = format!("{trigger:?}, mode {index}, pinned {pinned:?}");
            check(v.audio(n), "audio", &context);
            check(v.aux(n), "aux", &context);
            frame += n;
        }
    }
}

/// Criterion C3: "`audio` and `aux` stay finite and within +-1 under random
/// macro and pitch modulation, with and without triggers (including triggers
/// faster than 100 Hz and held gates)": every control (the three macros
/// included, so every patch is visited) over its range, in each of the four
/// trigger modes, per 1024-frame block, per 32-frame block with per-sample
/// inputs, and per sample.
#[test]
fn six_op_fm_stays_bounded_under_random_modulation() {
    for (i, trigger) in TRIGGERS.iter().enumerate() {
        stress(0x51A0 + i as u64, 3.0, *trigger, &[]);
    }
}

/// Criterion C3: the same bound with `harmonics` held on each patch in turn,
/// so every patch meets the modulation, in each trigger mode.
#[test]
fn every_patch_stays_bounded_under_random_modulation() {
    for patch in 0..32 {
        let h = patch_harmonics(patch);
        for (i, trigger) in TRIGGERS.iter().enumerate() {
            stress(
                0xAC0 + (patch * 4 + i) as u64,
                0.2,
                *trigger,
                &[("harmonics", h, h)],
            );
        }
    }
}
