//! Criterion: "At maximum feedback, 0 dBFS input stays within +/-1.0 for
//! 60 s."
//!
//! Each mode renders 60 s at 48 kHz with `feedback` 1.0 for four 0 dBFS
//! inputs: a full-scale sine, a full-scale square, constant +1 (the DC-ish
//! worst case), and a stereo pair of opposite full-scale squares. Each input
//! runs with different grain settings so the loop is exercised across the
//! control range. Two taps on `trigger` early on give `looping_delay` a
//! delay time (and fire a grain / restart playback in the other modes).
//!
//! These are long by design: 4 x 60 s per mode.

use super::rig::*;
use super::*;

const FRAMES: usize = 60 * SR as usize;

fn square(len: usize, period: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|n| {
            if (n / (period / 2)) % 2 == 0 {
                amp
            } else {
                -amp
            }
        })
        .collect()
}

fn run(mode: f32) {
    let sine = sine(FRAMES, 997.0, 1.0);
    let sq = square(FRAMES, 960, 1.0);
    let inverted: Vec<f32> = sq.iter().map(|v| -v).collect();
    // Outside granular mode `texture` is a tone filter that high-passes above
    // 0.5, which would remove the DC this case pushes through the loop; keep
    // those modes at or below neutral so DC passes. Granular mode keeps 0.9 so
    // the case still covers diffusion.
    let dc_texture = if mode == GRANULAR { 0.9 } else { 0.5 };
    let cases: [(&str, Feed, Vec<(&str, f32)>); 4] = [
        ("sine", Feed::mono(sine), vec![]),
        (
            "square",
            Feed::mono(sq.clone()),
            vec![
                ("pitch", 7.0),
                ("size", 0.3),
                ("density", 0.9),
                ("spread", 1.0),
            ],
        ),
        (
            "dc",
            Feed::mono(vec![1.0; FRAMES]),
            vec![("pitch", -12.0), ("texture", dc_texture), ("density", 0.2)],
        ),
        (
            "stereo squares",
            Feed {
                right: Some(inverted),
                ..Feed::mono(sq)
            },
            vec![
                ("position", 0.0),
                ("size", 1.0),
                ("pitch", 24.0),
                ("density", 1.0),
            ],
        ),
    ];
    for (input, feed, controls) in cases {
        let taps = gates(FRAMES, [s(0.5), s(0.7)], 48);
        let mut start = vec![("mode", mode), ("feedback", 1.0)];
        start.extend(controls);
        let config = json!({ "buffer_length": 1.0, "seed": 11 });
        let (l, r) = render_granular(config, &start, &[], &feed.with_trigger(taps));
        for (name, ch) in [("left", &l), ("right", &r)] {
            let worst = ch.iter().position(|v| !v.is_finite() || v.abs() > 1.0);
            assert!(
                worst.is_none(),
                "{input} {name}: {} at frame {} ({:.1} s)",
                ch[worst.unwrap_or(0)],
                worst.unwrap_or(0),
                worst.unwrap_or(0) as f64 / f64::from(SR)
            );
        }
        // Not vacuous: something was actually heard.
        assert!(peak(&l).max(peak(&r)) > 0.05, "{input}: output is silent");
    }
}

#[test]
fn granular_stays_within_unity_at_full_feedback_for_60_seconds() {
    run(GRANULAR);
}

#[test]
fn stretch_stays_within_unity_at_full_feedback_for_60_seconds() {
    run(STRETCH);
}

#[test]
fn looping_delay_stays_within_unity_at_full_feedback_for_60_seconds() {
    run(LOOPING_DELAY);
}
