//! Tape glide in `looping_delay`: the `glide` and `inertia` controls.
//!
//! Criteria (interface spec, `glide` and `inertia` rows): glide 0 is exactly
//! the crossfade jump; gliding never clicks (largest step within 1.5x steady
//! state), turning it on, off or changing it mid-move included; read speed
//! stays within 0.5x-2x; a leap beyond what the glide can cover still
//! crossfades; higher `inertia` makes a move take longer.
//!
//! Assumption: `position` maps to the delay roughly as position x
//! `buffer_length` (position 0.05 of a 4 s buffer is about 0.2 s). The moves
//! are sized so that holds for any plausible mapping: small moves stay inside
//! what the glide can cover, leaps are far beyond it (and the tests measure
//! the delay rather than assume it).

use super::rig::*;
use super::*;

const BUF: f64 = 4.0;
const EVENT: usize = 240_000; // 5 s
const TONE: f64 = 1000.0;

fn base(glide: Option<f32>, inertia: Option<f32>) -> Vec<(&'static str, f32)> {
    let mut v = vec![
        ("mode", LOOPING_DELAY),
        ("density", 0.0),
        ("position", 0.25),
    ];
    v.extend(glide.map(|g| ("glide", g)));
    v.extend(inertia.map(|i| ("inertia", i)));
    v
}

fn config() -> Value {
    json!({ "buffer_length": BUF, "seed": 1 })
}

fn moves(list: &[(f64, f32)]) -> Vec<Event> {
    let at = |t: f64| s(t) / BLOCK * BLOCK;
    list.iter().map(|&(t, p)| (at(t), "position", p)).collect()
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

/// "At 0 every change of delay is a short crossfade jump, exactly as without
/// the control", and "[inertia] has no effect with `glide` 0".
#[test]
fn glide_zero_is_bit_identical_to_the_control_left_alone() {
    let feed = Feed::mono(noise(s(6.0), 0.5, 5));
    let events = moves(&[(2.0, 0.6), (3.0, 0.3), (3.5, 0.32), (4.0, 0.9), (5.0, 0.1)]);
    let run = |start: Vec<(&str, f32)>, extra: &[Event]| {
        let all: Vec<Event> = events.iter().chain(extra).copied().collect();
        render_granular(config(), &start, &all, &feed)
    };
    let reference = run(base(None, None), &[]);
    assert!(peak(&reference.0) > 0.05, "silent output");
    let touched = [(s(1.5), "inertia", 0.0), (s(3.2), "inertia", 1.0)]
        .map(|(f, k, v)| (f / BLOCK * BLOCK, k, v));
    for (label, got) in [
        ("glide 0 set", run(base(Some(0.0), None), &[])),
        ("glide 0, inertia 1", run(base(Some(0.0), Some(1.0)), &[])),
        (
            "glide 0, inertia moved",
            run(base(Some(0.0), None), &touched),
        ),
        ("inertia alone", run(base(None, Some(0.0)), &touched)),
    ] {
        assert_eq!(bits(&got.0), bits(&reference.0), "{label}: left differs");
        assert_eq!(bits(&got.1), bits(&reference.1), "{label}: right differs");
    }
}

/// Largest step around an event vs steady state (the step-continuity
/// criterion): the larger of the largest steps in a window before and a
/// window after, on both channels.
fn assert_no_click(
    label: &str,
    out: &(Vec<f32>, Vec<f32>),
    pre: (usize, usize),
    win: (usize, usize),
    post: (usize, usize),
) {
    for (name, ch) in [("left", &out.0), ("right", &out.1)] {
        let steady = max_step(&ch[pre.0..pre.1]).max(max_step(&ch[post.0..post.1]));
        let around = max_step(&ch[win.0..win.1]);
        assert!(steady > 1e-5, "{label} {name}: steady state is silent");
        assert!(
            around <= 1.5 * steady,
            "{label} {name}: step {around} vs steady-state {steady} (x{:.2})",
            around / steady
        );
    }
}

fn smooth_run(start: &[(&str, f32)], events: &[Event], secs: f64) -> (Vec<f32>, Vec<f32>) {
    render_granular(config(), start, events, &Feed::mono(smooth(s(secs))))
}

/// Gliding does not click: a slow sweep of `position` (0.08 of the buffer
/// over 3 s, retargeting every block) and a moderate single move.
#[test]
fn gliding_a_sweep_or_a_moderate_move_does_not_click() {
    let sweep: Vec<Event> = (0..s(3.0) / BLOCK)
        .map(|k| {
            (
                EVENT + k * BLOCK,
                "position",
                0.25 + 0.08 * (k * BLOCK) as f32 / s(3.0) as f32,
            )
        })
        .collect();
    let out = smooth_run(&base(Some(0.5), None), &sweep, 11.0);
    let w = (EVENT - s(1.0), EVENT + s(4.5));
    assert_no_click(
        "slow sweep",
        &out,
        (EVENT - s(1.0), EVENT),
        w,
        (EVENT + s(5.0), EVENT + s(6.0)),
    );
    for inertia in [0.0, 0.5, 1.0] {
        let out = smooth_run(
            &base(Some(2.0), Some(inertia)),
            &moves(&[(5.0, 0.30)]),
            10.0,
        );
        let label = format!("moderate move, inertia {inertia}");
        assert_no_click(
            &label,
            &out,
            (EVENT - s(1.0), EVENT),
            (EVENT - 1, EVENT + s(3.5)),
            (EVENT + s(4.0), EVENT + s(5.0)),
        );
    }
}

/// "Turning `glide` on or off, or changing it, never clicks", mid-move.
#[test]
fn turning_glide_on_off_or_changing_it_mid_move_does_not_click() {
    let mut events = moves(&[(5.0, 0.30), (6.6, 0.36)]);
    for (t, v) in [
        (5.3, 0.0),
        (6.5, 1.5),
        (6.9, 0.3),
        (7.2, 0.0),
        (7.6, 2.0),
        (7.8, 0.0),
    ] {
        events.push((s(t) / BLOCK * BLOCK, "glide", v));
    }
    events.sort_by_key(|e| e.0);
    let out = smooth_run(&base(Some(1.5), None), &events, 11.0);
    assert_no_click(
        "glide toggled mid-move",
        &out,
        (EVENT - s(1.0), EVENT),
        (EVENT - 1, EVENT + s(3.0)),
        (EVENT + s(4.0), EVENT + s(5.0)),
    );
}

/// Windowed read speed of a steady tone of `TONE` Hz: (window start frame,
/// speed) from interpolated upward zero crossings in 20 ms windows every 5 ms.
fn speeds(x: &[f32]) -> Vec<(usize, f32)> {
    let ups: Vec<f64> = (1..x.len())
        .filter(|&n| x[n - 1] <= 0.0 && x[n] > 0.0)
        .map(|n| n as f64 - 1.0 + f64::from(-x[n - 1] / (x[n] - x[n - 1])))
        .collect();
    let (w, hop) = (s(0.02), s(0.005));
    (0..x.len() - w)
        .step_by(hop)
        .filter_map(|a| {
            let lo = ups.partition_point(|t| *t < a as f64);
            let hi = ups.partition_point(|t| *t < (a + w) as f64);
            (hi - lo >= 3).then(|| {
                let cycles = (hi - lo - 1) as f64;
                (
                    a,
                    (cycles / (ups[hi - 1] - ups[lo]) * f64::from(SR) / TONE) as f32,
                )
            })
        })
        .collect()
}

fn tone_trace(glide: f32, inertia: f32, list: &[(f64, f32)], secs: f64) -> Vec<(usize, f32)> {
    let feed = Feed::mono(sine(s(secs), TONE, 0.5));
    let (l, _) = render_granular(
        config(),
        &base(Some(glide), Some(inertia)),
        &moves(list),
        &feed,
    );
    speeds(&l)
}

/// "Read speed always stays between 0.5x and 2x", at several inertia values,
/// for a move to a longer delay (slower than 1x) and back (faster than 1x).
#[test]
fn read_speed_stays_within_half_to_double_during_glides() {
    for inertia in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let trace = tone_trace(2.0, inertia, &[(3.0, 0.30), (9.0, 0.25)], 15.0);
        let after = |t: f64| trace.iter().filter(move |(a, _)| *a >= s(t));
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &(a, r) in after(1.5) {
            assert!(
                (0.48..=2.05).contains(&r),
                "inertia {inertia}: speed {r} at {:.2} s",
                a as f64 / 48e3
            );
            let in_up = (s(3.0)..s(9.0)).contains(&a);
            if in_up {
                lo = lo.min(r)
            } else if a >= s(9.0) {
                hi = hi.max(r)
            }
        }
        assert!(
            lo < 0.97,
            "inertia {inertia}: no slow-down while the delay grew ({lo})"
        );
        assert!(
            hi > 1.03,
            "inertia {inertia}: no speed-up while the delay shrank ({hi})"
        );
    }
}

/// Correlation lag (frames) at which `y[at..at+480]` best matches `x`, and
/// its normalised score.
fn delay_of(x: &[f32], y: &[f32], at: usize) -> (usize, f32) {
    let w = 480;
    let mut sq = vec![0.0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        sq[i + 1] = sq[i] + f64::from(v * v);
    }
    let ey: f64 = y[at..at + w].iter().map(|v| f64::from(v * v)).sum();
    let mut best = (0, 0.0f32);
    for d in 0..=s(BUF).min(at) {
        let xs = &x[at - d..at - d + w];
        let dot: f32 = xs.iter().zip(&y[at..at + w]).map(|(a, b)| a * b).sum();
        let ex = sq[at - d + w] - sq[at - d];
        let score = (f64::from(dot) / (ex * ey).sqrt().max(1e-9)) as f32;
        if score > best.1 {
            best = (d, score);
        }
    }
    best
}

/// A move larger than the glide can cover crossfades: no pitch bend after the
/// crossfade, and the new delay is there within 50 ms. `cover` is the spec's
/// least of a quarter of the longest delay, 1 s and half the glide.
fn check_leap(from: f32, to: f32, glide: f32, inertia: f32) {
    let cover = (BUF / 4.0).min(1.0).min(f64::from(glide) / 2.0);
    let label = format!("leap {from}->{to}, glide {glide}, inertia {inertia}");
    let list = [(5.0, to)];
    let mut start = base(Some(glide), Some(inertia));
    start.push(("position", from)); // the later write wins
    let x = noise(EVENT + s(2.0), 0.5, 11);
    let (y, _) = render_granular(config(), &start, &moves(&list), &Feed::mono(x.clone()));
    let (old, old_score) = delay_of(&x, &y, EVENT - s(0.1));
    let (new, new_score) = delay_of(&x, &y, EVENT + s(1.5));
    let (early, early_score) = delay_of(&x, &y, EVENT + s(0.04));
    assert!(
        old_score > 0.5 && new_score > 0.5,
        "{label}: no clear delay ({old_score}, {new_score})"
    );
    assert!(
        (new as f64 - old as f64).abs() / 48e3 > cover,
        "{label}: test premise, delays {old} -> {new} frames"
    );
    assert!(
        early_score > 0.5,
        "{label}: no clear delay 50 ms after the leap ({early_score})"
    );
    assert!(
        early.abs_diff(new) <= 2,
        "{label}: delay {early} frames 50 ms after, {new} when settled"
    );
    let feed = Feed::mono(sine(EVENT + s(2.0), TONE, 0.5));
    let (l, _) = render_granular(config(), &start, &moves(&list), &feed);
    for (a, r) in speeds(&l)
        .into_iter()
        .filter(|(a, _)| *a >= EVENT + s(0.05))
    {
        assert!(
            (r - 1.0).abs() < 0.05,
            "{label}: speed {r} at +{:.0} ms",
            (a - EVENT) as f64 / 48.0
        );
    }
}

/// "A move larger than the glide can cover ... still crossfades".
#[test]
fn leaps_beyond_the_glide_still_crossfade() {
    for inertia in [0.0, 0.5, 1.0] {
        check_leap(0.1, 0.9, 2.0, inertia);
    }
    check_leap(0.25, 0.45, 0.5, 0.5);
}

/// "[inertia] Higher, the read speed is slower to rise and to fall, so a move
/// takes longer to reach the new delay and bends more gently."
/// Assumption: "reached" is when the read speed has returned to within 3% of
/// 1x for good; neighbouring inertia values may tie within 10 ms.
#[test]
fn higher_inertia_makes_a_move_take_longer_and_bend_less() {
    let rows: Vec<(f32, f64, f32)> = [0.0, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .map(|&inertia| {
            let trace = tone_trace(0.5, inertia, &[(3.0, 0.28)], 10.0);
            let after: Vec<_> = trace.iter().filter(|(a, _)| *a >= s(3.0)).collect();
            let last = after.iter().rev().find(|(_, r)| (r - 1.0).abs() > 0.03);
            let last =
                last.unwrap_or_else(|| panic!("inertia {inertia}: the move never bent the pitch"));
            assert!(last.0 < s(9.0), "inertia {inertia}: never settled");
            let peak = after
                .iter()
                .fold(0.0f32, |m, (_, r)| m.max((r - 1.0).abs()));
            (inertia, (last.0 - s(3.0)) as f64 / 48e3, peak)
        })
        .collect();
    for w in rows.windows(2) {
        assert!(
            w[1].1 >= w[0].1 - 0.01,
            "move time not monotonic in inertia: {rows:?}"
        );
    }
    assert!(
        rows[4].1 > rows[0].1 + 0.05,
        "inertia 1 is not slower than 0: {rows:?}"
    );
    assert!(
        rows[4].2 < rows[0].2,
        "inertia 1 does not bend more gently than 0: {rows:?}"
    );
}
