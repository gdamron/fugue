//! Criterion "it applies in the main graph and inside developments".

use super::*;

fn check(label: &str, wrapped: &Doc, flat: &Doc, d: usize) {
    for block in [64, 16] {
        let (wl, wr) = offline(wrapped, LOOP, block);
        let (fl, fr) = offline(flat, LOOP, block);
        let echoes = echo_offsets(&wl, &wr, 0, LOOP);
        assert!(echoes.len() >= 6, "{label}: echoes at {echoes:?}");
        assert_eq!(
            echoes[..6],
            exact(d, 6)[..],
            "{label} block {block}: exact loop"
        );
        assert_bits(
            &wl,
            &fl,
            &format!("{label} block {block} left, development vs flat"),
        );
        assert_bits(
            &wr,
            &fr,
            &format!("{label} block {block} right, development vs flat"),
        );
    }
}

const LOOP: usize = 60_000;

/// A loop inside a development has exactly the delay's length, and renders
/// bit for bit what the same wiring renders flat in the main graph.
#[test]
fn loop_inside_a_development_is_exact_and_matches_flat() {
    for min_time in [None, Some(0.01)] {
        let delay = delay_cfg(0.0625, min_time);
        let flat = gate_loop(delay.clone(), 0.7, 30.0);
        check(
            &format!("inside, min_time {min_time:?}"),
            &loop_inside(delay),
            &flat,
            3000,
        );
    }
}

/// A loop that runs from the main graph's delay through a development's
/// inner graph and back is exact too, and equals flat.
#[test]
fn loop_through_a_development_is_exact_and_matches_flat() {
    for min_time in [None, Some(0.01)] {
        let delay = delay_cfg(0.0625, min_time);
        let flat = gate_loop(delay.clone(), 0.7, 30.0);
        check(
            &format!("through, min_time {min_time:?}"),
            &loop_through(delay),
            &flat,
            3000,
        );
    }
}

/// Inside a development too: a module in the development's loop drives the
/// delay's `time_mod`, so it runs per sample (time + 1), and still equals the
/// same wiring flat.
#[test]
fn time_mod_fed_from_inside_a_development_falls_back_to_per_sample() {
    let delay = delay_cfg(0.0625, Some(0.01));
    let def = definition(
        vec![
            module("d", "delay", delay.clone()),
            module("loop", "vca", json!({ "level": 0.7 })),
            module("m", "vca", json!({ "level": 0.0 })),
        ],
        vec![
            conn("d", "audio", "loop", "audio"),
            conn("loop", "audio", "d", "audio"),
            conn("d", "audio", "m", "audio"),
            conn("m", "audio", "d", "time_mod"),
        ],
        ("d", "audio"),
    );
    let wrapped = with_development(gated(30.0), "echo_voice", def)
        .add("voice", "echo_voice", json!({}))
        .wire("src", "audio", "voice", "audio")
        .wire("voice", "audio", "dac", "audio_right");
    let flat = patch_mod(with_mod_tap(gate_loop(delay, 0.7, 30.0)));
    for block in [64, 16] {
        let (wl, wr) = offline(&wrapped, LOOP, block);
        let (fl, fr) = offline(&flat, LOOP, block);
        let echoes = echo_offsets(&wl, &wr, 0, LOOP);
        assert_eq!(echoes[..6], per_sample(3000, 6)[..], "block {block}");
        assert_bits(&wl, &fl, "left");
        assert_bits(&wr, &fr, "right");
    }
}
