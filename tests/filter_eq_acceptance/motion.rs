//! Stability under audio-rate cutoff modulation, and click-free glides of
//! live `boost`, `q` and `cutoff` writes in the new modes.

use crate::{filter, max_diff, max_step, noise, sine, Write, AMP, MS, SR};
use serde_json::{json, Value};

/// "Modulating cutoff from 20 Hz to 16 kHz at a 1 kHz rate with max Q: no
/// NaN, and output stays within ±4." The `cutoff` input is driven by
/// 20·800^((1 + sin 2π·1000t)/2) Hz, an exponential sweep across the whole
/// 20 Hz–16 kHz range a thousand times a second. Max Q is `q` 18 for the new
/// modes (with a +12 dB boost, the spec's largest tested boost) and
/// `resonance` 1.0 for lowpass / highpass / bandpass, at both slopes. Inputs
/// are 0.25-amplitude noise and a 0.25-amplitude 441.3 Hz sine.
#[test]
fn audio_rate_cutoff_sweep_stays_finite_and_bounded() {
    let len = SR as usize;
    let cutoff: Vec<f32> = (sine(len, 1000.0, 1.0).iter())
        .map(|s| 20.0 * 800f32.powf((1.0 + s) / 2.0))
        .collect();
    let mut configs: Vec<Value> = ["bell", "lowshelf", "highshelf", "notch"]
        .iter()
        .map(|mode| json!({ "filter_type": mode, "cutoff": 1000.0, "q": 18.0, "boost": 12.0 }))
        .collect();
    for mode in ["lowpass", "highpass", "bandpass"] {
        for slope in ["12db", "24db"] {
            configs.push(json!({ "filter_type": mode, "resonance": 1.0, "slope": slope }));
        }
    }
    let inputs = [noise(len, AMP, 0xfeed_beef), sine(len, 441.3, AMP)];
    let mut misses = Vec::new();
    for config in &configs {
        for (i, input) in inputs.iter().enumerate() {
            let out = filter(config.clone()).run_with(input, &[], Some(&cutoff));
            if let Some(n) = out.iter().position(|v| !v.is_finite() || v.abs() > 4.0) {
                misses.push(format!("{config} input {i}: sample {n} is {}", out[n]));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "unbounded output:\n{}",
        misses.join("\n")
    );
}

/// How far `out` has moved from `from` towards `to` over a window, as the
/// least-squares mix fraction (0 = still `from`, 1 = arrived at `to`).
pub fn fraction(out: &[f32], from: &[f32], to: &[f32]) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for ((&o, &a), &b) in out.iter().zip(from).zip(to) {
        let d = f64::from(b) - f64::from(a);
        num += (f64::from(o) - f64::from(a)) * d;
        den += d * d;
    }
    num / den
}

/// Asserts a transition written at `at` has not arrived by `by_ms`: in the
/// millisecond before `at + by_ms` the output is still measurably short of
/// `to` (an instant change reads 1.0 there). With `by_ms` = 5 this is "spread
/// over at least 5 ms" for a fade that is linear or faster at its start.
pub fn assert_spread(label: &str, out: &[f32], from: &[f32], to: &[f32], at: usize, by_ms: usize) {
    let w = at + (by_ms - 1) * MS..at + by_ms * MS;
    let f = fraction(&out[w.clone()], &from[w.clone()], &to[w]);
    assert!(
        f < 0.97,
        "{label}: {f:.3} of the way there {by_ms} ms after the write"
    );
}

/// A live write's output steps no more than 1.5x the larger of the two
/// steady renders it moves between (no zipper, no click).
pub fn assert_no_click(label: &str, out: &[f32], refs: &[&[f32]]) {
    let bound = refs.iter().map(|r| max_step(r)).fold(0.0, f32::max) * 1.5;
    let (step, pos) = out.windows(2).enumerate().fold((0.0, 0), |m, (i, w)| {
        let s = (w[1] - w[0]).abs();
        if s > m.0 {
            (s, i)
        } else {
            m
        }
    });
    assert!(
        step <= bound,
        "{label}: step {step} at {pos} exceeds {bound}"
    );
}

/// Renders a 997 Hz (non-integer period) tone through `config`, writing
/// `key = value` at 0.5 s; returns (input, output, steady render at
/// `config`, steady render at `target`, write frame).
fn glide(config: Value, target: Value, write: Write) -> [Vec<f32>; 4] {
    let input = sine(SR as usize, 997.0, AMP);
    let out = filter(config.clone()).run(&input, &[write]);
    let from = filter(config).run(&input, &[]);
    let to = filter(target).run(&input, &[]);
    [input, out, from, to]
}

/// "Writes to the new controls glide": live `boost` and `q` writes on a
/// running bell or shelf do not click, have not arrived 2 ms after the write
/// (they glide rather than jump; the pinned 10 ms glide shape is not
/// assumed), and arrive at the written value's response within 30 ms.
#[test]
fn live_boost_and_q_writes_glide() {
    let at = SR as usize / 2 + 7;
    let bell = |q: f32, boost: f32| json!({ "filter_type": "bell", "cutoff": 1300.0, "q": q, "boost": boost });
    let shelf = |boost: f32| json!({ "filter_type": "lowshelf", "cutoff": 2500.0, "boost": boost });
    let cases = [
        (
            "bell boost 0 -> 12",
            bell(2.0, 0.0),
            bell(2.0, 12.0),
            (at, "boost", 12.0),
        ),
        (
            "bell q 0.7 -> 8",
            bell(0.7, 12.0),
            bell(8.0, 12.0),
            (at, "q", 8.0),
        ),
        (
            "lowshelf boost -12 -> 12",
            shelf(-12.0),
            shelf(12.0),
            (at, "boost", 12.0),
        ),
    ];
    for (label, config, target, write) in cases {
        let [_, out, from, to] = glide(config, target, write);
        assert_no_click(label, &out, &[&from, &to]);
        assert_spread(label, &out, &from, &to, at, 2);
        let late = at + 30 * MS..;
        let miss = max_diff(&out[late.clone()], &to[late]);
        assert!(
            miss < 1e-3,
            "{label}: still {miss} from the target 30 ms on"
        );
    }
}

/// "Coefficient changes from modulated parameters must not zipper": a live
/// `cutoff` write that sweeps a +12 dB bell through the tone does not click.
#[test]
fn live_cutoff_write_on_a_bell_does_not_click() {
    let at = SR as usize / 2 + 7;
    let bell = |f0: f32| json!({ "filter_type": "bell", "cutoff": f0, "q": 2.0, "boost": 12.0 });
    let [input, out, _, _] = glide(bell(200.0), bell(5000.0), (at, "cutoff", 5000.0));
    let loudest = filter(bell(997.0)).run(&input, &[]);
    assert_no_click("bell cutoff 200 -> 5000", &out, &[&loudest]);
}
