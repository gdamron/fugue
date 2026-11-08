//! "Changing `filter_type` while audio plays crossfades from the old
//! response to the new over about 10 ms; the largest sample-to-sample step
//! across the switch is no more than 1.5× the steady-state largest step of
//! either response on a sustained saw."
//!
//! The source is a band-limited (additive) 110 Hz saw, so its own reset
//! does not dominate the steps. Each switch is made at the frame where the
//! old and new responses differ most, the worst moment for an instant cut.
//! References are fresh filters of each type run over the whole saw.

// These checks are written `!(measured <= limit)` on purpose: a NaN
// measurement must count as a miss, which `measured > limit` would let pass.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use crate::{filter, max_diff, max_step, render, saw, type_position, Write};
use serde_json::{json, Value};

const SR: u32 = 48_000;
const MS: usize = SR as usize / 1000;
const F0: f64 = 110.0;
/// Where the switch search starts: long after every response has settled.
const SETTLED: usize = 300 * MS;

/// The switches covered: classic to classic, classic to EQ and back.
const PAIRS: [(&str, &str); 6] = [
    ("lowpass", "highpass"),
    ("highpass", "bandpass"),
    ("bandpass", "lowpass"),
    ("lowpass", "bell"),
    ("bell", "highpass"),
    ("highpass", "lowshelf"),
];

fn config(mode: &str, slope: &str) -> Value {
    json!({ "filter_type": mode, "cutoff": 1000.0, "resonance": 0.5,
            "boost": 12.0, "q": 1.0, "slope": slope })
}

fn source() -> Vec<f32> {
    saw(SR, 600 * MS, F0, 4000.0, 0.5)
}

struct Switch {
    from: Vec<f32>,
    to: Vec<f32>,
    out: Vec<f32>,
    at: usize,
    label: String,
}

/// Renders `from` switched live to `to` at the frame (within one saw
/// period after settling) where the two responses differ most.
fn switch(from: &str, to: &str, slope: &str) -> Switch {
    let x = source();
    let from_ref = render(SR, &config(from, slope), &x);
    let to_ref = render(SR, &config(to, slope), &x);
    let period = (f64::from(SR) / F0) as usize;
    let at = (SETTLED..SETTLED + period)
        .max_by(|&a, &b| {
            let d = |i: usize| (from_ref[i] - to_ref[i]).abs();
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    let writes: [Write; 1] = [(at, "filter_type", type_position(to))];
    let out = filter(SR, &config(from, slope)).run(&x, &writes);
    Switch {
        from: from_ref,
        to: to_ref,
        out,
        at,
        label: format!("{from} -> {to} ({slope})"),
    }
}

fn cases() -> Vec<Switch> {
    let mut all: Vec<Switch> = PAIRS.iter().map(|(a, b)| switch(a, b, "12db")).collect();
    all.push(switch("lowpass", "highpass", "24db"));
    all
}

/// Largest step of a settled render.
fn steady_step(x: &[f32]) -> f32 {
    max_step(&x[100 * MS..])
}

#[test]
fn type_switch_steps_no_larger_than_either_response() {
    let mut misses = Vec::new();
    for s in cases() {
        let steady = steady_step(&s.from).max(steady_step(&s.to));
        let across = max_step(&s.out[s.at - 1..s.at + 30 * MS]);
        if !(across <= 1.5 * steady) {
            misses.push(format!(
                "{}: step {across:.4} across the switch, steady {steady:.4} (limit {:.4})",
                s.label,
                1.5 * steady
            ));
        }
    }
    assert!(
        misses.is_empty(),
        "type switches click:\n{}",
        misses.join("\n")
    );
}

/// "About 10 ms": 2-3 ms after the switch the output is still well short of
/// the new response, and from 15 ms on it is the new response.
#[test]
fn type_switch_fades_over_about_10_ms() {
    let mut misses = Vec::new();
    for s in cases() {
        let window = |a: usize, b: usize| s.at + a * MS..s.at + b * MS;
        let early = window(2, 3);
        let gap = max_diff(&s.from[early.clone()], &s.to[early.clone()]);
        let left = max_diff(&s.out[early.clone()], &s.to[early]);
        assert!(
            gap > 0.01,
            "{}: the responses differ early ({gap})",
            s.label
        );
        if !(left >= 0.3 * gap) {
            misses.push(format!(
                "{}: 2-3 ms after the switch already {left:.4} from the new response \
                 (responses differ by {gap:.4}); the fade finished too soon",
                s.label
            ));
        }
        let late = window(15, 60);
        let gap = max_diff(&s.from[late.clone()], &s.to[late.clone()]);
        let left = max_diff(&s.out[late.clone()], &s.to[late]);
        if !(left <= 0.01 * gap) {
            misses.push(format!(
                "{}: 15-60 ms after the switch still {left:.4} from the new response \
                 (responses differ by {gap:.4})",
                s.label
            ));
        }
    }
    assert!(misses.is_empty(), "type fades:\n{}", misses.join("\n"));
}

/// A second switch made while the first fade is still running is just as
/// smooth, and the output settles on the last type written.
#[test]
fn switch_during_a_fade_stays_smooth() {
    let x = source();
    let (a, b, c) = ("lowpass", "highpass", "bandpass");
    let refs: Vec<Vec<f32>> = [a, b, c]
        .iter()
        .map(|m| render(SR, &config(m, "12db"), &x))
        .collect();
    let (first, second) = (SETTLED, SETTLED + 4 * MS);
    let writes: [Write; 2] = [
        (first, "filter_type", type_position(b)),
        (second, "filter_type", type_position(c)),
    ];
    let out = filter(SR, &config(a, "12db")).run(&x, &writes);
    let steady = refs.iter().map(|r| steady_step(r)).fold(0.0, f32::max);
    let across = max_step(&out[first - 1..second + 30 * MS]);
    assert!(
        across <= 1.5 * steady,
        "{a} -> {b} -> {c}: step {across:.4} across the switches, steady {steady:.4}"
    );
    let late = second + 20 * MS..second + 60 * MS;
    let left = max_diff(&out[late.clone()], &refs[2][late.clone()]);
    let gap = max_diff(&refs[0][late.clone()], &refs[2][late]);
    assert!(
        left <= 0.01 * gap,
        "{a} -> {b} -> {c}: 20-60 ms after the last switch still {left:.4} from {c}"
    );
}
