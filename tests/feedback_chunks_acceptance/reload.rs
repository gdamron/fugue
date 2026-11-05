//! Criterion "a live reload that adds a module to the loop, or changes the
//! delay's max_time, applies within one block with no click".
//!
//! A running invention is driven by hand, 64 frames at a time; the reload is
//! made between two blocks, so it must be heard in the very next one. The loop
//! is a sustained 220 Hz sine (level 0.3) through a delay of 48 samples
//! (chunks of 48 > 1 frame) and a vca at 0.5 outside the atom.
//!
//! "No click" here: the output never steps by more than 1.5 times the largest
//! step of the same invention rendered without the reload, except where the
//! spec itself removes signal (see the max_time test).

use super::*;

const PRE: usize = 40;
const POST: usize = 20;

fn sine_loop(max_time: Option<f64>, dry_on_right: bool) -> Doc {
    let mut cfg = json!({ "time": 0.001, "min_time": 0.001, "feedback": 0.0 });
    if let Some(max) = max_time {
        cfg["max_time"] = json!(max);
    }
    let doc = Doc::new()
        .add(
            "osc",
            "oscillator",
            json!({ "waveform": "sine", "frequency": 220.0 }),
        )
        .add("src", "vca", json!({ "level": 0.3 }))
        .add("d", "delay", cfg)
        .add("loop", "vca", json!({ "level": 0.5 }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("osc", "audio", "src", "audio")
        .wire("src", "audio", "d", "audio")
        .wire("d", "audio", "loop", "audio")
        .wire("loop", "audio", "d", "audio")
        .wire("d", "audio", "dac", "audio_left");
    if dry_on_right {
        doc.wire("src", "audio", "dac", "audio_right")
    } else {
        doc
    }
}

/// The loop with a unity-gain `probe` vca added between the delay and the
/// loop vca, and tapped to the right output.
fn with_probe(doc: Doc) -> Doc {
    doc.add("probe", "vca", json!({ "level": 1.0 }))
        .unwire("d", "audio", "loop", "audio")
        .wire("d", "audio", "probe", "audio")
        .wire("probe", "audio", "loop", "audio")
        .wire("probe", "audio", "dac", "audio_right")
}

/// Renders `blocks` of the baseline (no reload).
fn baseline(doc: &Doc, blocks: usize) -> (Vec<f32>, Vec<f32>) {
    let live = Live::start(doc);
    let out = live.render(blocks);
    live.stop();
    out
}

/// Adding a module to the loop is heard in the first block after the reload,
/// and not before.
#[test]
fn a_module_added_to_the_loop_is_heard_in_the_next_block() {
    let mut live = Live::start(&sine_loop(None, false));
    let (_, right_before) = live.render(PRE);
    live.reload(&with_probe(sine_loop(None, false)));
    let (left, right) = live.render(POST);
    live.stop();
    assert!(
        right_before.iter().all(|&v| v == 0.0),
        "the probe is silent before the reload"
    );
    let first = ..BLOCK;
    assert!(
        left[first].iter().any(|v| v.abs() > 1e-3),
        "the loop is sounding"
    );
    assert_bits(
        &right[first],
        &left[first],
        "the probe (unity) passes the delay's output in block 1",
    );
}

/// The same reload leaves the loop's output alone: no step beyond 1.5 times
/// the steady signal's, and (the inserted vca being unity) the very samples
/// of the render without the reload, so the loop kept its state and its length.
#[test]
fn a_module_added_to_the_loop_does_not_click_or_change_the_output() {
    let (steady, _) = baseline(&sine_loop(None, false), PRE + POST);
    let mut live = Live::start(&sine_loop(None, false));
    let (mut left, _) = live.render(PRE);
    live.reload(&with_probe(sine_loop(None, false)));
    left.extend(live.render(POST).0);
    live.stop();
    let limit = 1.5 * max_step(&steady);
    assert!(
        max_step(&left) <= limit,
        "a step of {} beats the limit {limit}",
        max_step(&left)
    );
    assert_bits(&left, &steady, "left, reloaded vs not reloaded");
}

/// Changing `max_time` rebuilds the delay (its line starts empty, as any
/// rebuilt module does). That must apply at the very next block and the loop
/// must then run exactly as a fresh one: the first 48 frames of the next block
/// are silent, after which the sine returns through the exact loop. Bounds:
/// the only step is the vanished wet signal (never more than its own peak),
/// and the oscillator, which is not rebuilt, keeps its phase.
///
/// Assumption: an implementation that changed `max_time` in place (keeping
/// the line) is also acceptable: then the output must continue unchanged,
/// bit for bit.
#[test]
fn changing_max_time_applies_in_the_next_block_and_the_loop_resumes_exactly() {
    let mut live = Live::start(&sine_loop(None, true));
    let (mut left, mut dry) = live.render(PRE);
    live.reload(&sine_loop(Some(2.0), true));
    let (l, r) = live.render(POST);
    live.stop();
    left.extend(l);
    dry.extend(r);

    let start = PRE * BLOCK;
    let (steady_left, steady_dry) = baseline(&sine_loop(None, true), PRE + POST);
    assert_bits(
        &dry,
        &steady_dry,
        "the oscillator is untouched by the reload",
    );
    assert_bits(
        &left[..start],
        &steady_left[..start],
        "nothing changes before the reload",
    );

    // A rebuilt delay: empty line, then the exact loop over the dry sine.
    let mut rebuilt = left.clone();
    for n in start..left.len() {
        let echo = n - 48;
        rebuilt[n] = if echo < start {
            0.0
        } else {
            dry[echo] + rebuilt[echo] * 0.5
        };
    }
    let in_place = left[start..] == steady_left[start..];
    if !in_place {
        assert_close(&left[start..], &rebuilt[start..], 1e-6, "after the rebuild");
        let peak = steady_left[..start]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            max_step(&left) <= peak + 1e-6,
            "step {} exceeds the wet signal's peak {peak}",
            max_step(&left)
        );
    }
}
