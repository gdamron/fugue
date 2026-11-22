//! A cable patched into an audio input ramps in over ten milliseconds
//! rather than stepping in; one into a gate, a clock or a pitch takes at
//! once, as before.

use serde_json::{json, Value};

use super::{Onsets, MAX_ONSETS};
use crate::rpc::StructuralEdit;
use crate::RenderEngine;

mod catalog;

/// Ten milliseconds at the tests' 48 kHz.
const RAMP: usize = 480;

/// A still square wave: a constant 1 for the first seconds.
fn still(id: &str) -> Value {
    json!({ "id": id, "type": "oscillator",
            "config": { "frequency": 0.001, "waveform": "square" } })
}

/// `modules` and `connections` with a dac, in blocks of 128 frames.
fn engine(modules: Value, connections: Value) -> RenderEngine {
    let mut modules = modules.as_array().unwrap().clone();
    modules.push(json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }));
    let invention = json!({ "version": "1.0.0", "modules": modules, "connections": connections });
    let mut engine = RenderEngine::new(48_000);
    engine.set_block_size(128);
    engine.load_json(&invention.to_string()).unwrap();
    engine
}

/// Still squares `a` and `b` (1), with vca `half` holding `a` at 0.5 and
/// vca `cv` holding `b` at 0.25, and only `half` heard: the dac plays 0.5.
fn two_sources() -> RenderEngine {
    engine(
        json!([still("a"), still("b"),
               { "id": "half", "type": "vca", "config": { "level": 0.5 } },
               { "id": "cv", "type": "vca", "config": { "level": 0.25 } }]),
        json!([
            { "from": "a", "from_port": "audio", "to": "half", "to_port": "audio" },
            { "from": "b", "from_port": "audio", "to": "cv", "to_port": "audio" },
            { "from": "half", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]),
    )
}

/// The left channel of the next `frames` frames.
fn render(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out.chunks(2).map(|frame| frame[0]).collect()
}

/// Applies `edits` as one change, as an `apply_edits` batch does.
fn edit(engine: &RenderEngine, edits: Value) {
    let edits: Vec<StructuralEdit> = serde_json::from_value(edits).unwrap();
    engine.apply_edits(&edits).unwrap();
}

/// The largest change from one frame to the next, `before` the first.
fn largest_step(before: f32, out: &[f32]) -> f32 {
    let mut last = before;
    let mut largest = 0.0f32;
    for &x in out {
        largest = largest.max((x - last).abs());
        last = x;
    }
    largest
}

/// Whether `x` is `to`, but for rounding: a still square plays 1 less an
/// ulp.
fn near(x: f32, to: f32) -> bool {
    (x - to).abs() < 1e-6
}

/// `out` moves evenly from `from` to `to` over the ramp, and stays.
fn assert_ramps(from: f32, to: f32, out: &[f32]) {
    let even = (to - from).abs() / RAMP as f32;
    let largest = largest_step(from, out);
    assert!(largest <= even * 1.01, "largest step {largest} past {even}");
    let half = (from + to) / 2.0;
    assert!(
        (out[RAMP / 2 - 1] - half).abs() < 0.01,
        "no ramp: {:?}",
        &out[..4]
    );
    for (i, &x) in out.iter().enumerate().skip(RAMP - 1) {
        assert!(near(x, to), "frame {i}: {x}, not at {to}");
    }
}

#[test]
fn a_cable_patched_into_an_audio_input_ramps_in() {
    let mut engine = engine(json!([still("a")]), json!([]));
    assert!(render(&mut engine, 1_024).iter().all(|&x| x == 0.0));
    engine.connect("a", "audio", "dac", "audio").unwrap();
    assert_ramps(0.0, 1.0, &render(&mut engine, 2_048));
}

#[test]
fn a_cable_added_to_a_summed_audio_input_ramps_in_on_top() {
    let mut engine = two_sources();
    assert!(render(&mut engine, 1_024).iter().all(|&x| near(x, 0.5)));
    engine.connect("cv", "audio", "dac", "audio").unwrap();
    assert_ramps(0.5, 0.75, &render(&mut engine, 2_048));
}

#[test]
fn a_cable_into_a_control_input_takes_at_once() {
    // `voice` plays `a` at its own level, 1, until `cv` is patched in.
    let mut engine = engine(
        json!([still("a"), still("b"),
               { "id": "cv", "type": "vca", "config": { "level": 0.25 } },
               { "id": "voice", "type": "vca" }]),
        json!([
            { "from": "b", "from_port": "audio", "to": "cv", "to_port": "audio" },
            { "from": "a", "from_port": "audio", "to": "voice", "to_port": "audio" },
            { "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]),
    );
    assert!(render(&mut engine, 1_024).iter().all(|&x| near(x, 1.0)));
    engine.connect("cv", "audio", "voice", "level").unwrap();
    let out = render(&mut engine, 1_024);
    assert!(out.iter().all(|&x| near(x, 0.25)), "{:?}", &out[..4]);
}

#[test]
fn a_cable_unpatched_as_it_ramps_in_glides_out_from_where_it_was() {
    let mut engine = engine(json!([still("a")]), json!([]));
    render(&mut engine, 1_024);
    engine.connect("a", "audio", "dac", "audio").unwrap();
    let mut out = render(&mut engine, 256);
    engine.disconnect("a", "audio", "dac", "audio").unwrap();
    out.extend(render(&mut engine, 2_048));
    assert!(largest_step(0.0, &out) <= 1.0 / RAMP as f32 * 1.01);
    assert!(out[256 + RAMP..].iter().all(|&x| x == 0.0));
}

#[test]
fn a_source_removed_as_it_ramps_in_glides_out_from_where_it_was() {
    let mut engine = two_sources();
    render(&mut engine, 1_024);
    engine.connect("cv", "audio", "dac", "audio").unwrap();
    let mut out = render(&mut engine, 256);
    engine.remove_module("cv").unwrap();
    out.extend(render(&mut engine, 2_048));
    assert!(largest_step(0.5, &out) <= 0.25 / RAMP as f32 * 1.01);
    assert!(out[256 + RAMP..].iter().all(|&x| near(x, 0.5)));
}

#[test]
fn a_ramp_carries_on_through_an_edit_that_moves_its_modules() {
    // `z` ahead of the rest, so removing it moves both ends of the cable.
    let mut engine = engine(json!([still("z"), still("a")]), json!([]));
    render(&mut engine, 1_024);
    engine.connect("a", "audio", "dac", "audio").unwrap();
    let mut out = render(&mut engine, 128);
    engine.remove_module("z").unwrap();
    out.extend(render(&mut engine, 2_048));
    assert_ramps(0.0, 1.0, &out);
}

#[test]
fn a_mixer_channel_unpatched_and_patched_again_before_it_plays_carries_on() {
    let mut engine = engine(
        json!([still("a"),
               { "id": "mix", "type": "mixer",
                 "config": { "channel_count": 1, "levels": [0.5], "master": 1.0 } }]),
        json!([
            { "from": "a", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
            { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio" }
        ]),
    );
    let before = *render(&mut engine, 1_024).last().unwrap();
    engine.disconnect("a", "audio", "mix", "audio.0").unwrap();
    engine.connect("a", "audio", "mix", "audio.0").unwrap();
    let out = render(&mut engine, 1_024);
    assert!(
        largest_step(before, &out) < 1e-6,
        "{before} -> {:?}",
        &out[..4]
    );
}

#[test]
fn a_mixer_channel_patched_in_ramps_in_once() {
    let mut engine = engine(
        json!([still("a"), still("b"),
               { "id": "mix", "type": "mixer",
                 "config": { "channel_count": 2, "levels": [0.5, 0.5], "master": 1.0 } }]),
        json!([
            { "from": "a", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
            { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio" }
        ]),
    );
    let before = *render(&mut engine, 1_024).last().unwrap();
    engine.connect("b", "audio", "mix", "audio.1").unwrap();
    let out = render(&mut engine, 2_048);
    let after = *out.last().unwrap();
    assert!(after > before, "{before} -> {after}");
    // The mixer adds no ramp of its own, so the two do not compound.
    assert_ramps(before, after, &out);
}

#[test]
fn an_idle_module_inserted_into_a_chain_passes_on_what_it_hears() {
    // `pass`, a unity vca, is in the piece but feeds nothing: nothing has
    // heard it, so what it hears steps in rather than ramping from silence.
    let mut engine = engine(
        json!([still("a"), { "id": "pass", "type": "vca" }]),
        json!([{ "from": "a", "from_port": "audio", "to": "dac", "to_port": "audio" }]),
    );
    let before = *render(&mut engine, 1_024).last().unwrap();
    edit(
        &engine,
        json!([
            { "op": "disconnect", "from": "a", "from_port": "audio", "to": "dac", "to_port": "audio" },
            { "op": "connect", "from": "a", "from_port": "audio", "to": "pass", "to_port": "audio" },
            { "op": "connect", "from": "pass", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]),
    );
    let out = render(&mut engine, 2_048);
    assert!(
        largest_step(before, &out) < 1e-6,
        "{before} -> {:?}",
        &out[..4]
    );
}

#[test]
fn a_right_input_that_played_the_left_takes_its_cable_at_once() {
    // Unpatched, the reverb's right input plays its left: patching the same
    // source in changes nothing heard, where a ramp from silence would dip.
    let piece = || {
        engine(
            json!([still("a"), { "id": "verb", "type": "reverb" }]),
            json!([
                { "from": "a", "from_port": "audio", "to": "verb", "to_port": "audio_left" },
                { "from": "verb", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
                { "from": "verb", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
            ]),
        )
    };
    let (mut edited, mut untouched) = (piece(), piece());
    let stereo = |engine: &mut RenderEngine| {
        let mut out = vec![0.0; 2_048 * 2];
        engine.render_interleaved(&mut out).unwrap();
        out
    };
    assert_eq!(stereo(&mut edited), stereo(&mut untouched));
    edited.connect("a", "audio", "verb", "audio_right").unwrap();
    assert_eq!(stereo(&mut edited), stereo(&mut untouched));
}

#[test]
fn an_edit_that_patches_no_heard_audio_changes_nothing_heard() {
    let mut edited = two_sources();
    let mut untouched = two_sources();
    let mut heard = render(&mut edited, 1_000);
    let mut reference = render(&mut untouched, 1_000);
    edited
        .add_module("idle", "oscillator", &json!({ "frequency": 330.0 }))
        .unwrap();
    edited.add_module("sink", "vca", &json!({})).unwrap();
    edited.connect("idle", "audio", "sink", "audio").unwrap();
    edited.connect("cv", "audio", "sink", "level").unwrap();
    heard.extend(render(&mut edited, 4_000));
    reference.extend(render(&mut untouched, 4_000));
    assert!(heard == reference, "the edit changed what is heard");
}

#[test]
fn a_cable_past_the_slots_steps_in() {
    let mut onsets = Onsets::default();
    for i in 0..MAX_ONSETS {
        assert!(onsets.begin(0, i, (1, 0), 0));
    }
    assert!(!onsets.begin(0, MAX_ONSETS, (1, 0), 0));
    // A cable that is not ramping is heard whole.
    assert_eq!(onsets.take(0, MAX_ONSETS, (1, 0), 10), 1.0);
    // One that is was heard as far as its ramp had come.
    let heard = onsets.take(0, 0, (1, 0), 240);
    assert!((heard - 240.0 / 480.0).abs() < 1e-6, "{heard}");
}
