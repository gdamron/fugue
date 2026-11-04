//! Criterion: "no sample exceeds the ceiling, on noise at +12 dB and on sine
//! bursts", and the gain comes down without clicks.

use super::rig::*;
use super::*;

/// Asserts no output sample exceeds 10^(ceiling/20). Assumption: one part in
/// 1e6 of float rounding is allowed on the bound.
fn assert_under(out: &[Vec<f32>], ceiling: f64, what: &str) {
    let limit = (db_to_amp(ceiling) * (1.0 + 1e-6)) as f32;
    for (side, x) in out.iter().enumerate() {
        let (n, v) =
            (x.iter().enumerate()).fold(
                (0, 0.0f32),
                |m, (n, v)| if v.abs() > m.1 { (n, v.abs()) } else { m },
            );
        assert!(
            v <= limit,
            "{what}: side {side} sample {n} is {v} > {limit}"
        );
    }
}

/// White noise peaking at +12 dBFS (amplitude 3.98), different on each side,
/// for several ceilings, releases and lookaheads.
#[test]
fn noise_at_plus_12_db_never_exceeds_the_ceiling() {
    let amp = db_to_amp(12.0) as f32;
    let (l, r) = (noise(amp, 1), noise(amp, 2));
    for (ceiling, release, lookahead) in
        [(-1.0, 0.1, 0.005), (-6.0, 0.001, 0.0005), (-0.1, 2.0, 0.02)]
    {
        let config = json!({ "ceiling": ceiling, "release": release, "lookahead": lookahead });
        let mut lim = build("limiter", config);
        let out = lim.run(secs(1.0), &[("audio_left", &l), ("audio_right", &r)]);
        assert_under(
            &out,
            ceiling,
            &format!("noise, ceiling {ceiling}, release {release}"),
        );
    }
}

/// 1 kHz and 80 Hz sine bursts at +6 and +12 dB with abrupt onsets (40 ms
/// on, 120 ms off), on one side only and on both, with a fast release so the
/// gain has recovered fully before each burst.
#[test]
fn sine_bursts_never_exceed_the_ceiling() {
    for (db, freq) in [(6.0, 1000.0), (12.0, 1000.0), (12.0, 80.0)] {
        let s = sine(db_to_amp(db), freq);
        let burst = move |t: usize| {
            if t % secs(0.16) < secs(0.04) {
                s(t + 7)
            } else {
                0.0
            }
        };
        let mut lim = build("limiter", json!({ "ceiling": -1.0, "release": 0.01 }));
        let out = lim.run(secs(1.0), &[("audio_left", &burst)]);
        assert_under(&out, -1.0, &format!("{db} dB {freq} Hz bursts, left only"));
        let mut lim = build("limiter", json!({ "ceiling": -3.0, "release": 0.01 }));
        let out = lim.run(
            secs(1.0),
            &[("audio_left", &burst), ("audio_right", &burst)],
        );
        assert_under(&out, -3.0, &format!("{db} dB {freq} Hz bursts, both"));
    }
}

/// "The gain comes down smoothly over the lookahead ahead of a peak (no
/// clicks)": a DC step from 0.5 to 4.0 is limited by a gain (output over the
/// delayed input) that never moves more than three times its full depth
/// spread evenly over the lookahead in one sample. Assumption: DC is limited
/// like any other signal (no DC blocking).
#[test]
fn gain_comes_down_smoothly_ahead_of_a_peak() {
    let lookahead = secs(0.005);
    let step = |t: usize| if t < secs(0.2) { 0.5 } else { 4.0 };
    let mut lim = build("limiter", json!({ "ceiling": -1.0, "lookahead": 0.005 }));
    let out = lim.run(secs(0.3), &[("audio_left", &step)]);
    let gain: Vec<f32> = (lookahead..out[0].len())
        .map(|n| out[0][n] / step(n - lookahead))
        .collect();
    let depth = 1.0 - db_to_amp(-1.0) as f32 / 4.0;
    let limit = 3.0 * depth / lookahead as f32;
    assert!(
        max_step(&gain) <= limit,
        "gain step {} > {limit}",
        max_step(&gain)
    );
    assert!(gain[secs(0.25)] < 0.25, "the step is limited");
}

/// A `ceiling` change glides (about 20 ms) without clicking: a 220 Hz sine at
/// 0.5 with the ceiling dropped from -1 to -12 dB live steps no more than
/// the sine itself does, and afterwards respects the new ceiling.
#[test]
fn a_ceiling_change_glides_without_a_click() {
    let s = sine(0.5, 220.0);
    let mut lim = build("limiter", json!({ "ceiling": -1.0 }));
    lim.run(secs(0.2), &[("audio_left", &s)]);
    lim.set("ceiling", -12.0);
    let out = lim.run(secs(0.4), &[("audio_left", &s)]);
    let steady = (0.5 * std::f64::consts::TAU * 220.0 / f64::from(SR)) as f32;
    let step = max_step(&out[0]);
    assert!(
        step <= 1.1 * steady + 0.002,
        "step {step} > sine's {steady}"
    );
    let after = (db_to_amp(-12.0) * (1.0 + 1e-6)) as f32;
    assert!(peak(&out[0][secs(0.1)..]) <= after, "new ceiling holds");
}
