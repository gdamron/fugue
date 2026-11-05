//! Criteria "ping-pong at 375 ms" and "the loop length is exactly the delay".

use super::*;

const LOOP_FRAMES: usize = 90_000;

/// Loop length: a loop patched outside the delay (delay.audio -> vca ->
/// delay.audio) repeats exactly `time` apart, with no extra sample, for the
/// default `min_time` (chunks of one frame) and for `min_time` giving longer
/// chunks (smaller than, and equal to, `time`).
#[test]
fn outside_loop_repeats_exactly_the_delay_time_apart() {
    for min_time in [None, Some(0.01), Some(0.0625)] {
        let doc = gate_loop(delay_cfg(0.0625, min_time), 0.7, 30.0);
        let (left, right) = offline(&doc, LOOP_FRAMES, 64);
        let echoes = echo_offsets(&left, &right, 0, LOOP_FRAMES);
        assert!(
            echoes.len() >= 8,
            "min_time {min_time:?}: echoes at {echoes:?}"
        );
        assert_eq!(echoes[..8], exact(3000, 8)[..], "min_time {min_time:?}");
    }
}

/// Loop length: the same at other block sizes (the chunk never exceeds a
/// block, so this exercises chunks cut short by the block's end).
#[test]
fn outside_loop_is_exact_at_any_block_size() {
    for block in [1, 16, 64, 256] {
        let doc = gate_loop(delay_cfg(0.0625, Some(0.01)), 0.7, 30.0);
        let (left, right) = offline(&doc, LOOP_FRAMES, block);
        let echoes = echo_offsets(&left, &right, 0, LOOP_FRAMES);
        assert_eq!(echoes[..8], exact(3000, 8)[..], "block {block}");
    }
}

/// Two delays in a cross-feedback loop through vcas (a ping-pong).
pub fn ping_pong(delay: Value, gain: f64) -> Doc {
    gated(10.0)
        .add("a", "delay", delay.clone())
        .add("b", "delay", delay)
        .add("ab", "vca", json!({ "level": gain }))
        .add("ba", "vca", json!({ "level": gain }))
        .wire("src", "audio", "a", "audio")
        .wire("a", "audio", "ab", "audio")
        .wire("ab", "audio", "b", "audio")
        .wire("b", "audio", "ba", "audio")
        .wire("ba", "audio", "a", "audio")
        .wire("a", "audio", "dac", "audio_left")
        .wire("b", "audio", "dac", "audio_right")
}

/// Ping-pong at 375 ms (dotted 1/8 at 120 bpm, or `time` 0.375): left carries
/// the dry gate and delay A, right delay B. Taken together the gate and every
/// repeat are exactly 18,000 samples apart, pass after pass.
#[test]
fn ping_pong_at_375_ms_repeats_every_18000_samples() {
    let configs = [
        json!({ "time": 0.375, "feedback": 0.0 }),
        json!({ "division": "dotted_8th_note", "bpm": 120.0, "feedback": 0.0 }),
    ];
    for cfg in configs {
        let (left, right) = offline(&ping_pong(cfg.clone(), 0.8), 200_000, 64);
        let mut edges = rising_edges(&left, EDGE);
        edges.extend(rising_edges(&right, EDGE));
        edges.sort_unstable();
        let start = edges[0];
        assert!(edges.len() >= 10, "{cfg}: edges at {edges:?}");
        let want: Vec<usize> = (0..10).map(|k| start + 18_000 * k).collect();
        assert_eq!(edges[..10], want[..], "{cfg}: gate and repeats");
    }
}
