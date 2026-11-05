//! Criterion 5: bounded, finite, normal output under random modulation.

use super::rig::*;
use super::*;
use fugue::dsp::Rng;

/// One way of driving the module: frames per `process` call, and whether the
/// inputs change every sample (they are held for the block otherwise).
/// Controls change on every call.
struct Mode {
    block: usize,
    per_sample: bool,
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + rng.unit() * (hi - lo)
}

fn randomize_controls(r: &mut Reso, rng: &mut Rng, skip_model: bool) {
    for meta in r.controls.clone() {
        if skip_model && meta.key == "model" {
            continue;
        }
        let value = match &meta.kind {
            // Integer-valued controls (polyphony) get whole numbers.
            ControlKind::Number { min, max, .. } if meta.key == "polyphony" => {
                uniform(rng, *min, *max + 0.999).floor().min(*max)
            }
            ControlKind::Number { min, max, .. } => uniform(rng, *min, *max),
            ControlKind::Bool => (rng.next_u32() & 1) as f32,
            ControlKind::String { options: Some(o) } => (rng.next_u32() as usize % o.len()) as f32,
            ControlKind::String { options: None } => continue,
        };
        r.set(&meta.key, value);
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

/// Drives a fresh resonator with random controls and inputs for `secs`
/// seconds, split 40/40/20 between three modes; fixes `model` when given.
/// `audio_gain` is the amplitude of the white noise on `audio` (0 leaves
/// `audio` unconnected, so the internal exciter strikes on `strum`).
fn stress(seed: u64, secs: f64, fixed_model: Option<usize>, audio_gain: f32) {
    let mut r = reso(json!({}));
    let mut rng = Rng::new(seed);
    r.connect(r.frequency_in);
    if audio_gain > 0.0 {
        r.connect_audio();
    }
    if let Some(position) = fixed_model {
        r.select(position);
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
    let (freq, strum, audio) = (r.frequency_in, r.strum_in, r.audio_in);
    let mut high_left = 0u32;
    let mut frame = 0;
    for (index, (mode, share)) in modes.iter().enumerate() {
        let calls = (seconds(secs) as f64 * share) as usize / mode.block;
        for _ in 0..calls {
            randomize_controls(&mut r, &mut rng, fixed_model.is_some());
            let n = mode.block;
            let held = (
                audible(&mut rng),
                uniform(&mut rng, -audio_gain, audio_gain),
            );
            for i in 0..n {
                let (f, a) = if mode.per_sample {
                    (
                        audible(&mut rng),
                        uniform(&mut rng, -audio_gain, audio_gain),
                    )
                } else {
                    held
                };
                // Strum: random-length pulses, so edges land anywhere.
                let s = if high_left > 0 {
                    high_left -= 1;
                    1.0
                } else if rng.unit() < 0.003 {
                    high_left = rng.next_u32() % 64;
                    1.0
                } else {
                    0.0
                };
                r.input_block(freq)[i] = f;
                r.input_block(strum)[i] = s;
                if audio_gain > 0.0 {
                    r.input_block(audio)[i] = a;
                }
            }
            r.module.module_mut().process(n);
            for (name, x) in [
                ("audio", r.audio(n)),
                ("odd", r.odd(n)),
                ("even", r.even(n)),
            ] {
                check(x, name, index, frame);
            }
            frame += n;
        }
    }
}

/// Criterion: "Under random full-range modulation of every control and the
/// inputs for a few seconds, output stays in [-1, 1], finite, no
/// subnormals": every declared control over its range (`model` and
/// `polyphony` included, so models and voice counts change constantly),
/// `frequency` input 20 Hz to 20 kHz, strum pulses of random length; per
/// 1024-frame block, per 32-frame block with per-sample inputs, and per
/// sample; fixed seed; 6 s; internal exciter.
#[test]
fn random_modulation_with_the_internal_exciter_stays_bounded() {
    stress(0x5E50, 6.0, None, 0.0);
}

/// Criterion: the same with white noise at full scale ([-1, 1]) on `audio`,
/// which switches the internal exciter off.
#[test]
fn random_modulation_with_noise_on_audio_stays_bounded() {
    stress(0xA0D10, 6.0, None, 1.0);
}

/// Criterion: the same with the model held on each in turn, so every model
/// meets the full-range modulation rather than a crossfade of two.
#[test]
fn each_model_stays_bounded_under_random_modulation() {
    let count = reso(json!({})).models.len();
    for position in 0..count {
        stress(0xAB5 + position as u64, 3.0, Some(position), 0.0);
        stress(0xCD7 + position as u64, 3.0, Some(position), 1.0);
    }
}

/// Criterion: the output bound holds for a hot excitation too.
/// Assumption: INTERFACE.md's "Output stays within [-1, 1]" is unconditional,
/// so noise at 8x full scale on `audio` is still bounded.
#[test]
fn hot_excitation_stays_bounded() {
    stress(0x407, 4.0, None, 8.0);
}

/// Criterion: the same bound with all numeric controls at their minimum,
/// then at their maximum (polyphony 1, then 4), `frequency` input at each
/// end of the audible range, and a strike every 100 ms, for each model.
#[test]
fn extremes_stay_bounded() {
    let count = reso(json!({})).models.len();
    for position in 0..count {
        for at_max in [false, true] {
            for hz in [20.0f32, 20_000.0] {
                let mut r = reso(json!({}));
                r.select(position);
                for meta in r.controls.clone() {
                    if let ControlKind::Number { min, max, .. } = meta.kind {
                        r.set(&meta.key, if at_max { max } else { min });
                    }
                }
                r.hold_frequency(hz);
                let mut frame = 0;
                for _ in 0..15 {
                    r.strike();
                    r.run(seconds(0.1), BLOCK, |r, n| {
                        for (name, x) in [
                            ("audio", r.audio(n)),
                            ("odd", r.odd(n)),
                            ("even", r.even(n)),
                        ] {
                            check(x, name, position, frame);
                        }
                        frame += n;
                    });
                }
            }
        }
    }
}
