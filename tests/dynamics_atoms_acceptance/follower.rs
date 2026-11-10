//! The `follower` atom: a stereo-linked peak detector with a sidechain.

use super::rig::*;
use super::*;

/// The settled envelope of constant inputs, after 2 s.
fn settled(config: Value, inputs: &[Input]) -> f32 {
    *build("follower", config).run(secs(2.0), inputs)[0]
        .last()
        .unwrap()
}

/// First frame at or after `from` where `level` crosses `at` in the
/// direction `rising`, counted from `from`.
fn crossing(level: &[f32], from: usize, at: f32, rising: bool) -> usize {
    let hit = (from..level.len()).find(|&n| {
        if rising {
            level[n] >= at
        } else {
            level[n] <= at
        }
    });
    hit.unwrap_or_else(|| panic!("the envelope crosses {at}")) - from
}

/// Criterion: "attack and release (time to 63%) are within 10%": from a
/// steady 0 a step to 1 reaches 0.632 after attack x 48000 samples, and from
/// a steady 1 a step to 0 falls to 0.368 after release x 48000 samples.
#[test]
fn attack_and_release_reach_63_percent_on_time() {
    for (attack, release) in [(0.01, 0.1), (0.05, 0.5), (0.002, 0.02)] {
        let (up, down) = (secs(0.2), secs(0.2) + secs(3.0));
        let step = move |t: usize| if (up..down).contains(&t) { 1.0 } else { 0.0 };
        let config = json!({ "attack": attack, "release": release });
        let level = build("follower", config).run(down + secs(3.0), &[("audio_left", &step)]);
        for (from, at, rising, tau) in [(up, 0.632, true, attack), (down, 0.368, false, release)] {
            let (got, want) = (
                crossing(&level[0], from, at, rising) as f64,
                tau * f64::from(SR),
            );
            let what = if rising { "attack" } else { "release" };
            assert!(
                (got - want).abs() <= 0.1 * want,
                "{what} {tau}: {got} samples, want {want}"
            );
        }
    }
}

/// "0 follows at once": attack 0 reaches a step's level on the step's sample.
#[test]
fn zero_attack_follows_at_once() {
    let step = |t: usize| if t >= 1000 { 0.75 } else { 0.0 };
    let level = build("follower", json!({ "attack": 0.0 })).run(2000, &[("audio_left", &step)]);
    assert!(
        (level[0][1000] - 0.75).abs() < 1e-6,
        "level {}",
        level[0][1000]
    );
}

/// `highpass` keeps lows from moving the envelope. Assumption: at 1 kHz a
/// 40 Hz sine's envelope falls below a fifth of its peak, while a 6 kHz sine
/// through a 200 Hz high-pass keeps its envelope within 15% of its peak.
#[test]
fn highpass_keeps_lows_out_of_the_envelope() {
    let (low, high) = (sine(0.5, 40.0), sine(0.5, 6000.0));
    let low_env = settled(json!({ "highpass": 1000.0 }), &[("audio_left", &low)]);
    assert!(low_env < 0.1, "40 Hz through a 1 kHz high-pass: {low_env}");
    let high_env = settled(json!({ "highpass": 200.0 }), &[("audio_left", &high)]);
    assert!(
        (high_env - 0.5).abs() < 0.075,
        "6 kHz through 200 Hz: {high_env}"
    );
}

/// "Non-finite input is treated as silence": NaN and infinities in a steady
/// 0.5 never push the envelope above 0.5 or out of the finite.
#[test]
fn non_finite_input_is_treated_as_silence() {
    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let dirty = move |t: usize| {
        if t % 500 == 3 {
            bad[(t / 500) % 3]
        } else {
            0.5
        }
    };
    let inputs: [Input; 3] = [
        ("audio_left", &dirty),
        ("audio_right", &dirty),
        ("sidechain", &dirty),
    ];
    let level = build("follower", json!({ "sidechain_mix": 0.5 })).run(secs(1.0), &inputs);
    assert!(
        level[0].iter().all(|v| v.is_finite() && *v <= 0.5 + 1e-6),
        "peak {}",
        peak(&level[0])
    );
}
