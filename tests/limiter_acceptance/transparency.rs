//! Criterion: "below threshold, output equals the input delayed by exactly
//! the stated lookahead in samples"; the gain is shared by both sides.

use super::rig::*;
use super::*;

/// Asserts `out` is `input` delayed by exactly `delay` samples, bit for bit,
/// with silence before it.
fn assert_delayed(out: &[f32], input: &dyn Fn(usize) -> f32, delay: usize, what: &str) {
    for (n, &y) in out.iter().enumerate() {
        let want = if n < delay { 0.0 } else { input(n - delay) };
        assert!(
            y.to_bits() == want.to_bits() || y == want,
            "{what}: sample {n} is {y}, want {want}"
        );
    }
}

/// Noise peaking just under the ceiling, on both sides, comes out delayed by
/// round(lookahead x 48000) samples: 240 for the default 5 ms, 24 and 960 at
/// the ends of the range, and the range clamps (0 -> 0.5 ms, 0.1 -> 20 ms).
#[test]
fn below_the_ceiling_the_output_is_the_input_delayed_by_the_lookahead() {
    let (l, r) = (noise(0.88, 3), noise(0.5, 4));
    for (lookahead, samples) in [
        (None, 240),
        (Some(0.005), 240),
        (Some(0.0025), 120),
        (Some(0.0005), 24),
        (Some(0.02), 960),
        (Some(0.0), 24),
        (Some(0.1), 960),
    ] {
        let config = match lookahead {
            Some(s) => json!({ "ceiling": -1.0, "lookahead": s }),
            None => json!({}),
        };
        let mut lim = build("limiter", config);
        let out = lim.run(secs(0.5), &[("audio_left", &l), ("audio_right", &r)]);
        assert_delayed(
            &out[0],
            &l,
            samples,
            &format!("left, lookahead {lookahead:?}"),
        );
        assert_delayed(
            &out[1],
            &r,
            samples,
            &format!("right, lookahead {lookahead:?}"),
        );
    }
}

/// A lower ceiling: a sine within +/-10^(-12/20) passes delayed and bit
/// exact with the ceiling at -12 dB; a mono source on `audio_left` alone
/// leaves the right side silent.
#[test]
fn a_mono_source_below_a_low_ceiling_is_delayed_exactly() {
    let s = sine(db_to_amp(-12.5), 997.0);
    let mut lim = build("limiter", json!({ "ceiling": -12.0, "lookahead": 0.01 }));
    let out = lim.run(secs(0.5), &[("audio_left", &s)]);
    assert_delayed(&out[0], &s, 480, "left");
    assert!(out[1].iter().all(|v| *v == 0.0), "right is silent");
}

/// "Each is its input delayed by exactly the lookahead, times one gain shared
/// by both sides": a loud left side pulls the quiet right side down by the
/// same gain.
#[test]
fn one_gain_is_shared_by_both_sides() {
    let delay = secs(0.005);
    let (l, r) = (sine(4.0, 300.0), sine(0.2, 1100.0));
    let mut lim = build("limiter", json!({ "ceiling": -1.0 }));
    let out = lim.run(secs(0.5), &[("audio_left", &l), ("audio_right", &r)]);
    let mut pulled = false;
    for n in secs(0.1)..out[0].len() {
        let (xl, xr) = (l(n - delay), r(n - delay));
        if xl.abs() > 0.5 && xr.abs() > 0.05 {
            let (gl, gr) = (out[0][n] / xl, out[1][n] / xr);
            assert!(
                (gl - gr).abs() < 1e-4,
                "sample {n}: left gain {gl}, right {gr}"
            );
            pulled |= gr < 0.5;
        }
    }
    assert!(pulled, "the quiet side is pulled down with the loud one");
}

/// "Non-finite input is treated as silence": NaN and infinities in a quiet
/// sine come out as zeros a lookahead later, everything else delayed exactly
/// (no gain reduction), and all output is finite.
#[test]
fn non_finite_input_is_treated_as_silence() {
    let clean = sine(0.3, 440.0);
    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let dirty = |t: usize| {
        if t % 1000 == 500 {
            bad[(t / 1000) % 3]
        } else {
            clean(t)
        }
    };
    let silenced = |t: usize| if t % 1000 == 500 { 0.0 } else { clean(t) };
    let mut lim = build("limiter", json!({}));
    let out = lim.run(
        secs(0.5),
        &[("audio_left", &dirty), ("audio_right", &dirty)],
    );
    for side in &out {
        assert!(side.iter().all(|v| v.is_finite()), "output is finite");
        assert_delayed(side, &silenced, 240, "non-finite as silence");
    }
}
