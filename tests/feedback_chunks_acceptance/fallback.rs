//! Criterion "a group with an emit-time input fed from inside the group, or
//! with a zero-delay cycle, falls back to per-sample and still renders
//! correctly". Correct is the per-sample behaviour: the loop repeats at
//! `time` + 1 sample per pass, the output is finite, and it does not depend on
//! the block size. (That it equals a per-sample render bit for bit is checked
//! in `feedback_chunks_kill_switch_acceptance`.)

use super::*;

const FRAMES: usize = 90_000;

fn echoes(doc: &Doc, block: usize) -> Vec<usize> {
    let (left, right) = offline(doc, FRAMES, block);
    echo_offsets(&left, &right, 0, FRAMES)
}

/// A module in the loop drives the delay's `time_mod` (with a value of
/// zero, so the delay time stays `time`): the cut module's emit-time input has
/// a source inside the group, so the group runs per sample and the loop is
/// time + 1. Sharp: the same wiring with `m` not patched is exact, so a
/// scheduler that chunks anyway reads 3000, 6000, ... and fails.
#[test]
fn time_mod_fed_from_inside_the_loop_repeats_at_time_plus_one() {
    for min_time in [None, Some(0.01)] {
        let base = with_mod_tap(gate_loop(delay_cfg(0.0625, min_time), 0.7, 30.0));
        let unpatched = echoes(&base, 64);
        assert_eq!(
            unpatched[..6],
            exact(3000, 6)[..],
            "unpatched, min_time {min_time:?}"
        );
        for block in [16, 64, 256] {
            let got = echoes(&patch_mod(base.clone()), block);
            let want = per_sample(3000, 6);
            assert_eq!(
                got[..6],
                want[..],
                "patched, min_time {min_time:?}, block {block}"
            );
        }
    }
}

/// With a live modulation (the in-loop module passes the signal on), the
/// per-sample result does not depend on the block size; a chunk that read
/// `time_mod` once for a whole block would.
#[test]
fn time_mod_fed_from_inside_the_loop_is_independent_of_block_size() {
    let doc = |patched: bool| {
        let cfg =
            json!({ "time": 0.0625, "min_time": 0.01, "feedback": 0.0, "time_mod_depth": 0.002 });
        let doc = Doc::new()
            .add("n", "noise", json!({ "seed": 5, "level": 0.2 }))
            .add("d", "delay", cfg)
            .add("loop", "vca", json!({ "level": 0.5 }))
            .add("m", "vca", json!({ "level": 1.0 }))
            .add("dac", "dac", json!({ "soft_clip": false }))
            .wire("n", "white", "d", "audio")
            .wire("d", "audio", "loop", "audio")
            .wire("loop", "audio", "d", "audio")
            .wire("d", "audio", "m", "audio")
            .wire("d", "audio", "dac", "audio_left")
            .wire("m", "audio", "dac", "audio_right");
        if patched {
            patch_mod(doc)
        } else {
            doc
        }
    };
    let (reference, _) = offline(&doc(true), 30_000, 64);
    assert!(reference.iter().all(|v| v.is_finite()));
    assert!(
        reference.iter().any(|v| v.abs() > 0.05),
        "the loop is audible"
    );
    for block in [16, 256, 1] {
        let (left, _) = offline(&doc(true), 30_000, block);
        assert_bits(&left, &reference, &format!("block {block} vs 64"));
    }
    let (plain, _) = offline(&doc(false), 30_000, 64);
    assert!(
        plain.iter().zip(&reference).any(|(a, b)| a != b),
        "the modulation is heard"
    );
}

/// A zero-delay cycle in the same group as the delay loop (the loop vca and a
/// second vca feed each other) means not every cycle passes through the delay:
/// per-sample, so the delay loop is time + 1. The second vca's gain is small
/// so the repeats' edges stay where per-sample scheduling puts them.
#[test]
fn a_zero_delay_cycle_beside_the_delay_loop_falls_back_to_per_sample() {
    for min_time in [None, Some(0.01)] {
        let doc = gate_loop(delay_cfg(0.0625, min_time), 0.7, 30.0)
            .add("side", "vca", json!({ "level": 0.01 }))
            .wire("loop", "audio", "side", "audio")
            .wire("side", "audio", "loop", "audio");
        for block in [16, 256] {
            let got = echoes(&doc, block);
            assert_eq!(
                got[..6],
                per_sample(3000, 6)[..],
                "min_time {min_time:?}, block {block}"
            );
        }
    }
}

/// A feedback group with no delay anywhere (two vcas feeding each other) still
/// renders: finite, audible, the same at every block size.
#[test]
fn a_cycle_with_no_delay_renders_finite_and_independent_of_block_size() {
    let doc = Doc::new()
        .add("n", "noise", json!({ "seed": 9, "level": 0.2 }))
        .add("a", "vca", json!({ "level": 0.6 }))
        .add("b", "vca", json!({ "level": 0.6 }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("n", "white", "a", "audio")
        .wire("a", "audio", "b", "audio")
        .wire("b", "audio", "a", "audio")
        .wire("a", "audio", "dac", "audio_left")
        .wire("b", "audio", "dac", "audio_right");
    let (l64, r64) = offline(&doc, 20_000, 64);
    assert!(l64.iter().chain(&r64).all(|v| v.is_finite()));
    assert!(l64.iter().any(|v| v.abs() > 0.05));
    for block in [16, 256] {
        let (l, r) = offline(&doc, 20_000, block);
        assert_bits(&l, &l64, &format!("left, block {block}"));
        assert_bits(&r, &r64, &format!("right, block {block}"));
    }
}
