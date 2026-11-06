//! Identities and measurable behaviour from the interface: `mix` 0 is the dry
//! input, `stereo_phase` 0 gives identical sides, an unpatched `audio_right`
//! copies the left, and a fixed chain is an allpass with a notch at its
//! corner.

use super::rig::*;
use super::*;

/// Assumption: config values are the starting values (no glide in from the
/// defaults at build), so `mix` 0 from config is exact from sample 0.
#[test]
fn mix_zero_outputs_each_input_sample_for_sample() {
    let (left, right) = (pad(secs(2.0), 110.0, 0.8), noise(secs(2.0), 0.7, 4));
    let config = json!({ "mix": 0.0, "depth": 1.0, "feedback": 0.9, "rate": 5.0 });
    let mut unit = build(config);
    let (l, r) = unit.run_with(
        &[("audio_left", &left), ("audio_right", &right)],
        &mut |_, _| {},
    );
    assert_eq!(l, left, "left is the dry input");
    assert_eq!(r, right, "right is the dry input");
}

/// Written live, `mix` glides to 0 and then the output is the dry input
/// exactly. A write applies at the first block boundary at or after 0.5 s;
/// the output is checked from 0.3 s after that block starts.
#[test]
fn mix_written_to_zero_settles_on_the_dry_input_exactly() {
    let x = pad(secs(2.0), 110.0, 0.8);
    let mut unit = build(json!({ "depth": 1.0, "feedback": 0.5 }));
    let mut written_at = None;
    let (l, r) = unit.run_hooked(&x, |t, u| {
        if written_at.is_none() && t >= secs(0.5) {
            written_at = Some(t);
            u.set("mix", 0.0);
        }
    });
    let written_at = written_at.expect("the write happened");
    let settled = written_at + secs(0.3);
    assert_eq!(l[settled..], x[settled..], "left");
    assert_eq!(r[settled..], x[settled..], "right copies the left source");
}

#[test]
fn stereo_phase_zero_gives_identical_sides() {
    let x = pad(secs(3.0), 110.0, 0.5);
    let config = json!({ "stereo_phase": 0.0, "depth": 1.0, "rate": 3.0, "feedback": 0.6 });
    let (l, r) = build(config).run_mono(&x);
    assert!(peak(&l) > 0.1);
    assert_eq!(l, r);
}

#[test]
fn the_default_stereo_phase_makes_the_sides_differ() {
    let x = pad(secs(3.0), 110.0, 0.5);
    let (l, r) = build(json!({ "depth": 1.0 })).run_mono(&x);
    assert!(rms_diff(&l, &r) > 0.05 * rms(&l), "sides differ");
}

/// With `audio_right` unpatched the right side copies the left: a mono
/// source on `audio_left` alone behaves as the same signal on both.
#[test]
fn an_unpatched_right_input_copies_the_left() {
    let x = pad(secs(2.0), 110.0, 0.5);
    let config = json!({ "depth": 1.0, "feedback": 0.4 });
    let (l1, r1) = build(config.clone()).run_mono(&x);
    let both = [("audio_left", &x[..]), ("audio_right", &x[..])];
    let (l2, r2) = build(config).run_with(&both, &mut |_, _| {});
    assert!(rms_diff(&l1, &l2) < 1e-6, "left");
    assert!(rms_diff(&r1, &r2) < 1e-6, "right");
}

/// A patched `audio_right` is processed on its own.
#[test]
fn a_patched_right_input_is_processed_separately_from_the_left() {
    let (a, b) = (sine(secs(1.0), 220.0, 0.5), sine(secs(1.0), 660.0, 0.5));
    let mut unit = build(json!({ "mix": 1.0, "depth": 0.0, "feedback": 0.0 }));
    let (l, r) = unit.run_with(&[("audio_left", &a), ("audio_right", &b)], &mut |_, _| {});
    let tail = secs(0.5);
    assert!(
        (rms(&l[tail..]) / rms(&a[tail..]) - 1.0).abs() < 0.01,
        "left stays 220 Hz"
    );
    assert!(
        (rms(&r[tail..]) / rms(&b[tail..]) - 1.0).abs() < 0.01,
        "right stays 660 Hz"
    );
    // The right output is the 660 Hz tone, not a copy of the left.
    assert!(rms_diff(&l[tail..], &r[tail..]) > 0.3 * rms(&l[tail..]));
}

/// `depth` 0 and `feedback` 0: a fixed allpass, so a steady sine keeps its
/// amplitude at `mix` 1 (within rounding), for any stage count.
#[test]
fn a_fixed_chain_at_full_mix_keeps_a_sines_amplitude() {
    for stage_count in [4, 6, 12] {
        for freq in [150.0, 700.0, 3_100.0] {
            let x = sine(secs(1.5), freq, 0.5);
            let config =
                json!({ "stage_count": stage_count, "depth": 0.0, "feedback": 0.0, "mix": 1.0 });
            let (l, r) = build(config).run_mono(&x);
            let tail = secs(0.5);
            for out in [&l, &r] {
                let ratio = rms(&out[tail..]) / rms(&x[tail..]);
                assert!(
                    (ratio - 1.0).abs() < 2e-3,
                    "{stage_count} stages at {freq} Hz: gain {ratio}"
                );
            }
        }
    }
}

/// Steady-state level of a sine through a fixed chain at `mix` 0.5.
fn notched_gain(stage_count: u32, corner: f32, freq: f64) -> f64 {
    let config = json!({ "stage_count": stage_count, "depth": 0.0, "feedback": 0.0,
        "mix": 0.5, "frequency": corner });
    let x = sine(secs(1.0), freq, 0.5);
    let (l, _) = build(config).run_mono(&x);
    let tail = secs(0.4);
    rms(&l[tail..]) / rms(&x[tail..])
}

/// At the corner frequency each stage is 90 degrees, so a chain of 6 stages
/// turns a sine at the corner by 540 degrees and cancels it against the dry
/// signal: the classic notch. The notch is searched within +/-7% of 700 Hz.
#[test]
fn a_fixed_six_stage_chain_at_half_mix_notches_a_sine_at_its_corner() {
    let gains: Vec<(f64, f64)> = (-7..=7)
        .map(|k| {
            let freq = 700.0 * (1.0 + f64::from(k) / 100.0);
            (freq, notched_gain(6, 700.0, freq))
        })
        .collect();
    let (freq, deepest) = gains.iter().fold(
        (0.0, f64::MAX),
        |m, &(f, g)| if g < m.1 { (f, g) } else { m },
    );
    assert!(
        deepest < 0.1,
        "deepest gain near 700 Hz is {deepest} at {freq} Hz: {gains:?}"
    );
    // Far from the notch the sine passes at about full level.
    assert!(notched_gain(6, 700.0, 60.0) > 0.7, "low sine passes");
}

/// The same with 4 stages: 360 degrees at the corner leaves it in phase (a
/// peak), and the notch sits where the chain turns 180 degrees,
/// at the corner times tan(45 deg / 2) = 0.414 -> about 290 Hz.
#[test]
fn a_fixed_four_stage_chain_at_half_mix_notches_below_its_corner() {
    let gains: Vec<(f64, f64)> = (-7..=7)
        .map(|k| {
            let freq = 290.0 * (1.0 + f64::from(k) / 100.0);
            (freq, notched_gain(4, 700.0, freq))
        })
        .collect();
    let (freq, deepest) = gains.iter().fold(
        (0.0, f64::MAX),
        |m, &(f, g)| if g < m.1 { (f, g) } else { m },
    );
    assert!(
        deepest < 0.1,
        "deepest gain near 290 Hz is {deepest} at {freq} Hz"
    );
    assert!(
        notched_gain(4, 700.0, 700.0) > 0.9,
        "the corner itself passes"
    );
}

/// Negative feedback inverts the loop, so notches and peaks swap places: at
/// the 6-stage notch (the corner) the two signs give clearly different
/// depths (about 0.21 against 0.37 of the input level for feedback +/-0.5
/// in the usual structure; only that they differ is asserted).
#[test]
fn feedback_sign_swaps_peaks_and_notches_at_the_corner() {
    let at = |feedback: f32| {
        let config = json!({ "stage_count": 6, "depth": 0.0, "feedback": feedback,
            "mix": 0.5, "frequency": 700.0 });
        let x = sine(secs(1.0), 700.0, 0.3);
        let (l, _) = build(config).run_mono(&x);
        rms(&l[secs(0.4)..]) / rms(&x[secs(0.4)..])
    };
    let (positive, negative) = (at(0.5), at(-0.5));
    assert!(
        (positive - negative).abs() > 0.05,
        "feedback sign changes the response: +{positive} / -{negative}"
    );
}
