//! The sweep follows `division` and `bpm` (control or input): with `division`
//! `quarter_note` at 120 bpm it repeats every 0.5 s.
//!
//! Measured with a steady sine whose period divides the sweep period in whole
//! samples, at `depth` 1 and `mix` 0.5, so the output level moves audibly
//! through the sweep. The output one sweep period later must match; one half
//! period later it must not (which also catches a sweep at twice the speed).

use super::rig::*;
use super::*;

/// RMS difference of `len` samples at `a` and at `b`, over the RMS at `a`.
fn mismatch(out: &[f32], a: usize, b: usize, len: usize) -> f64 {
    rms_diff(&out[a..a + len], &out[b..b + len]) / rms(&out[a..a + len])
}

/// Renders 3 s of a `tone` Hz sine through the phaser and asserts the output
/// repeats after `period` samples but not after half of it.
fn assert_sweep_period(config: Value, bpm_input: Option<f32>, tone: f64, period: usize) {
    let x = sine(secs(3.0), tone, 0.5);
    assert_eq!(
        period % (SR as f64 / tone) as usize,
        0,
        "test tone divides the period"
    );
    let mut unit = build(config.clone());
    let bpm = vec![bpm_input.unwrap_or(0.0); x.len()];
    let (l, r) = match bpm_input {
        Some(_) => unit.run_with(&[("audio_left", &x), ("bpm", &bpm)], &mut |_, _| {}),
        None => unit.run_mono(&x),
    };
    let (start, len) = (secs(1.0), period.min(secs(0.5)));
    for (side, out) in [("left", &l), ("right", &r)] {
        let same = mismatch(out, start, start + period, len);
        let half = mismatch(out, start, start + period / 2, len);
        assert!(
            same < 0.02,
            "{config} {side}: differs {same:.3} one period on"
        );
        assert!(
            half > 0.1,
            "{config} {side}: differs only {half:.3} half a period on"
        );
    }
}

#[test]
fn quarter_note_at_120_bpm_repeats_every_half_second() {
    let config = json!({ "division": "quarter_note", "bpm": 120.0, "depth": 1.0, "rate": 7.0 });
    assert_sweep_period(config, None, 1_000.0, 24_000);
}

#[test]
fn the_bpm_control_sets_the_synced_sweep_period() {
    let config = json!({ "division": "quarter_note", "bpm": 90.0, "depth": 1.0 });
    assert_sweep_period(config, None, 1_500.0, 32_000);
}

#[test]
fn a_dotted_eighth_at_100_bpm_repeats_every_0_45_seconds() {
    let config = json!({ "division": "dotted_8th_note", "bpm": 100.0, "depth": 1.0 });
    assert_sweep_period(config, None, 1_000.0, 21_600);
}

/// The `bpm` input overrides the `bpm` control while patched.
#[test]
fn the_bpm_input_overrides_the_bpm_control() {
    let config = json!({ "division": "quarter_note", "bpm": 120.0, "depth": 1.0 });
    assert_sweep_period(config, Some(240.0), 1_000.0, 12_000);
}

/// Without a division the sweep runs at `rate`, whatever the tempo: 2 Hz
/// repeats every 24,000 samples, and `bpm` has no effect.
#[test]
fn division_off_sweeps_at_rate_and_ignores_the_tempo() {
    let config = json!({ "rate": 2.0, "bpm": 300.0, "depth": 1.0 });
    assert_sweep_period(config, None, 1_000.0, 24_000);
}

/// Writing `bpm` while synced changes the speed without a step.
#[test]
fn a_tempo_change_while_synced_does_not_step_the_output() {
    let x = sine(secs(2.0), 300.0, 0.5);
    let config = json!({ "division": "quarter_note", "bpm": 60.0, "depth": 1.0 });
    let mut unit = build(config);
    unit.block = 64;
    let (l, r) = unit.run_hooked(&x, |t, u| {
        if t == secs(0.7) {
            u.set("bpm", 1_000.0);
        }
        if t == secs(1.4) {
            u.set("bpm", 1.0);
        }
    });
    for out in [&l, &r] {
        assert!(
            max_step(out) <= 1.5 * max_step(&x),
            "step {}",
            max_step(out)
        );
    }
}
