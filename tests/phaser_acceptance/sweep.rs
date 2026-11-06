//! Criterion 1: sweeping controls never steps the output. The largest
//! sample-to-sample step of the output (either side) is at most 1.5x the dry
//! input's largest step. Smooth sources (a sine and a pad) are used because
//! a click shows on them; noise's own steps would hide one.

use super::rig::*;
use super::*;

const BOUND: f32 = 1.5;

/// Runs `x` through a phaser built from `config`, with `hook` writing
/// controls each block, and asserts the step bound on both sides and that the
/// phaser actually produced sound.
fn assert_steps_bounded(
    config: Value,
    x: &[f32],
    block: usize,
    hook: impl FnMut(usize, &mut Unit),
) {
    let mut unit = build(config);
    unit.block = block;
    let (l, r) = unit.run_hooked(x, hook);
    let dry = max_step(x);
    for (side, out) in [("left", &l), ("right", &r)] {
        assert_finite(out, side);
        assert!(peak(out) > 0.2 * peak(x), "{side} is not silent");
        let (step, edge) = (max_step(out), max_edge_step(out, block));
        assert!(
            step <= BOUND * dry,
            "{side}: largest step {step} is {:.2}x the dry {dry}",
            step / dry
        );
        assert!(
            edge <= BOUND * dry,
            "{side}: block-edge step {edge} vs dry {dry}"
        );
    }
}

fn sources() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("sine 220", sine(secs(1.0), 220.0, 0.5)),
        ("pad", pad(secs(1.0), 110.0, 0.5)),
    ]
}

/// Rate 0.01 -> 10 Hz (geometric) and depth 0 -> 1 across one second.
#[test]
fn rate_and_depth_swept_up_over_one_second_do_not_step() {
    for (name, x) in sources() {
        for block in [64, 128] {
            println!("{name}, block {block}");
            assert_steps_bounded(json!({}), &x, block, |t, u| {
                let f = t as f32 / SR as f32;
                u.set("rate", 0.01 * 1000.0f32.powf(f));
                u.set("depth", f);
            });
        }
    }
}

#[test]
fn rate_and_depth_swept_down_over_one_second_do_not_step() {
    for (name, x) in sources() {
        println!("{name}");
        assert_steps_bounded(json!({ "rate": 10.0, "depth": 1.0 }), &x, 64, |t, u| {
            let f = t as f32 / SR as f32;
            u.set("rate", 10.0 * 0.001f32.powf(f));
            u.set("depth", 1.0 - f);
        });
    }
}

/// The worst case for a write: every block jumps between the range ends.
#[test]
fn rate_and_depth_jumping_between_their_extremes_every_block_do_not_step() {
    for (name, x) in sources() {
        println!("{name}");
        let mut high = false;
        assert_steps_bounded(json!({}), &x, 64, |_, u| {
            high = !high;
            u.set("rate", if high { 10.0 } else { 0.01 });
            u.set("depth", if high { 1.0 } else { 0.0 });
        });
    }
}

/// The interface says every number control glides; `frequency` and
/// `stereo_phase` are written across their whole range too.
#[test]
fn frequency_and_stereo_phase_jumping_every_block_do_not_step() {
    for (name, x) in sources() {
        println!("{name}");
        let mut high = false;
        assert_steps_bounded(json!({ "depth": 1.0 }), &x, 64, |_, u| {
            high = !high;
            u.set("frequency", if high { 4000.0 } else { 50.0 });
            u.set("stereo_phase", if high { 1.0 } else { 0.0 });
        });
    }
}

/// Assumption: a "fast feedback sweep" is feedback moved through -0.5 .. 0.5
/// four times a second (written every block) while the sweep runs at its
/// fastest. Feedback beyond +/-0.5 can legitimately gain a sine by more than
/// 1.5x at a resonance, which is level, not a step; the full +/-0.95 range is
/// covered by the headroom tests.
#[test]
fn a_fast_feedback_sweep_at_the_fastest_rate_does_not_step() {
    let x = sine(secs(1.0), 330.0, 0.4);
    assert_steps_bounded(json!({ "rate": 10.0, "depth": 1.0 }), &x, 64, |t, u| {
        let f = t as f32 / SR as f32;
        u.set("feedback", 0.5 * (std::f32::consts::TAU * 4.0 * f).sin());
    });
}

/// Feedback written straight between its extremes once per block: the level
/// may swing (so no step bound applies), but the output must stay finite and
/// within full scale.
#[test]
fn feedback_jumping_between_its_extremes_stays_bounded_and_finite() {
    let x = sine(secs(1.0), 330.0, 0.1);
    let mut high = false;
    let mut unit = build(json!({ "rate": 10.0, "depth": 1.0 }));
    unit.block = 64;
    let (l, r) = unit.run_hooked(&x, |_, u| {
        high = !high;
        u.set("feedback", if high { 0.95 } else { -0.95 });
    });
    for out in [&l, &r] {
        assert_finite(out, "output");
        assert!(peak(out) <= 1.0, "peak {}", peak(out));
    }
}

/// stage_count 4 -> 12 -> 4 while sweeping (it crossfades, so no click).
#[test]
fn a_stage_count_change_from_4_to_12_and_back_does_not_step() {
    for (name, x) in sources() {
        for block in [64, 100] {
            println!("{name}, block {block}");
            assert_steps_bounded(
                json!({ "stage_count": 4, "rate": 2.0, "depth": 1.0 }),
                &x,
                block,
                |t, u| {
                    if t >= secs(0.3) && t < secs(0.3) + block {
                        u.set("stage_count", 12.0);
                    }
                    if t >= secs(0.6) && t < secs(0.6) + block {
                        u.set("stage_count", 4.0);
                    }
                },
            );
        }
    }
}

/// Every stage count from 4 to 12 stepped one after another, and back.
#[test]
fn stepping_through_every_stage_count_does_not_step() {
    let x = pad(secs(2.2), 110.0, 0.5);
    let order: Vec<f32> = (4..=12).chain((4..12).rev()).map(|n| n as f32).collect();
    assert_steps_bounded(json!({ "rate": 2.0, "depth": 1.0 }), &x, 64, |t, u| {
        let slot = t / 6_000;
        if t % 6_000 < 64 && slot < order.len() {
            u.set("stage_count", order[slot]);
        }
    });
}
