//! Criterion 2: at every damping, a strike decays below -90 dBFS within the
//! stated ring time plus 1 s, stays there, and never grows.

use super::rig::*;
use super::stated::*;
use super::*;

const DAMPINGS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
/// RMS envelope window, in seconds, and how many make one growth segment.
const WINDOW: f64 = 0.05;
const WINDOWS_PER_SEGMENT: usize = 20;
/// How long the output is held to the bound after the deadline.
const HOLD: f64 = 2.0;
/// Absolute slack on envelope comparisons (RMS), far below audibility.
const SLACK: f32 = 1e-6;

/// Strikes `r` (already queued with `strike` or `impulse`) and checks the
/// decay: every sample of `audio`, `odd` and `even` is below -90 dBFS from
/// `ring + 1 s` to `ring + 1 s + HOLD`, zero or normal throughout, and the
/// 50 ms RMS envelope of `audio` does not grow after the attack:
///  - against the trailing second, no window exceeds 1.5x the trailing
///    maximum (catches fast growth), and
///  - the maximum of each 1 s segment is no more than 1.25x the previous
///    segment's (catches slow growth); beating between modes cannot trip it.
fn check_decay(mut r: Reso, model: &str, damping: f32, how: &str) {
    let ring = stated_ring(model, damping);
    assert!(ring.is_finite() && ring > 0.0, "{model}: ring {ring}");
    let deadline = seconds(f64::from(ring) + 1.0);
    let total = deadline + seconds(HOLD);
    let window = seconds(WINDOW);
    let mut env: Vec<f32> = Vec::new();
    let mut acc: Vec<f32> = Vec::with_capacity(window);
    let mut frame = 0usize;
    let mut loudest = 0.0f32;
    r.run(total, BLOCK, |r, n| {
        for (name, x) in [("audio", r.audio(n)), ("odd", r.odd(n)), ("even", r.even(n))] {
            for (i, v) in x.iter().enumerate() {
                assert!(
                    *v == 0.0 || v.is_normal(),
                    "{model} d={damping} ({how}): {name} non-finite or subnormal ({v:e}) at frame {}",
                    frame + i
                );
                if frame + i >= deadline {
                    assert!(
                        v.abs() < MINUS_90_DB,
                        "{model} d={damping} ({how}): {name} is {:.1} dBFS at {:.2} s, \
                         past the stated {ring} s + 1 s",
                        20.0 * v.abs().log10(),
                        (frame + i) as f64 / SRF
                    );
                }
            }
        }
        for &v in r.audio(n) {
            loudest = loudest.max(v.abs());
            acc.push(v);
            if acc.len() == window {
                env.push(rms(&acc));
                acc.clear();
            }
        }
        frame += n;
    });
    assert!(
        loudest > 1e-3,
        "{model} d={damping} ({how}): the strike is silent"
    );

    let k = env
        .iter()
        .enumerate()
        .fold((0, 0.0), |a, (i, v)| if *v > a.1 { (i, *v) } else { a })
        .0;
    for j in k + 1..env.len() {
        let from = j.saturating_sub(WINDOWS_PER_SEGMENT).max(k);
        let trailing = env[from..j].iter().fold(0.0f32, |m, v| m.max(*v));
        assert!(
            env[j] <= 1.5 * trailing + SLACK,
            "{model} d={damping} ({how}): envelope grows at {:.2} s ({} vs trailing max {})",
            j as f64 * WINDOW,
            env[j],
            trailing
        );
    }
    let seg: Vec<f32> = env
        .chunks(WINDOWS_PER_SEGMENT)
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(*v)))
        .collect();
    for (s, pair) in seg.windows(2).enumerate() {
        assert!(
            pair[1] <= 1.25 * pair[0] + SLACK,
            "{model} d={damping} ({how}): 1 s segment {} peaks at {} after {} in segment {s}",
            s + 1,
            pair[1],
            pair[0]
        );
    }
}

/// Criterion: "At every damping value, an impulse decays below -90 dBFS
/// within the stated time + 1 s, with no growth", for every model, struck
/// once with the internal exciter at 220 Hz. The bound is on the sample peak
/// of `audio`, `odd` and `even` (the stricter reading of "below -90 dBFS"),
/// held for 2 s after the deadline; "no growth" is as in `check_decay`.
/// Assumption: the stated time is for the default `structure`, `brightness`
/// and `position`, and the model's `frequency` of 220 Hz is representative.
#[test]
fn internal_exciter_decays_within_stated_time_at_every_damping() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for damping in DAMPINGS {
            let mut r = ready(position, 220.0, damping);
            r.strike();
            check_decay(r, model, damping, "internal exciter");
        }
    }
}

/// Criterion: the same, "once with a full-scale one-sample impulse on
/// `audio` instead of the internal exciter", for every model at the shortest
/// (0) and longest (1) damping. Assumption: with `audio` connected and no
/// strum, the impulse excites the active voice.
#[test]
fn impulse_on_audio_decays_within_stated_time() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for damping in [0.0f32, 1.0] {
            let mut r = ready(position, 220.0, damping);
            r.impulse();
            check_decay(r, model, damping, "impulse on audio");
        }
    }
}

/// Criterion (spec of `ring_seconds`): it is `None` for an unknown model,
/// positive and finite for each model, longest at damping 1, and "nothing
/// sustains forever" (an upper bound of 300 s).
#[test]
fn stated_ring_time_is_sane() {
    assert_eq!(stated_ring_for_unknown("no_such_model"), None);
    let names = reso(json!({})).models;
    for model in &names {
        let longest = stated_ring(model, 1.0);
        assert!(longest > 0.0 && longest < 300.0, "{model}: {longest} s");
        for damping in DAMPINGS {
            let t = stated_ring(model, damping);
            assert!(t > 0.0 && t.is_finite(), "{model} d={damping}: {t}");
            assert!(
                t <= longest,
                "{model}: damping {damping} lasts {t} > {longest}"
            );
        }
    }
}
