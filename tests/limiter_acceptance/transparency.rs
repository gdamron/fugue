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
    for (n, (&yl, &yr)) in out[0].iter().zip(&out[1]).enumerate().skip(secs(0.1)) {
        let (xl, xr) = (l(n - delay), r(n - delay));
        if xl.abs() > 0.5 && xr.abs() > 0.05 {
            let (gl, gr) = (yl / xl, yr / xr);
            assert!(
                (gl - gr).abs() < 1e-4,
                "sample {n}: left gain {gl}, right {gr}"
            );
            pulled |= gr < 0.5;
        }
    }
    assert!(pulled, "the quiet side is pulled down with the loud one");
}
