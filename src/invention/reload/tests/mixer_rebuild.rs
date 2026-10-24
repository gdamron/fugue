//! A reload that rebuilds a mixer while it plays glides from what the old
//! mixer was playing to the new one's levels, rather than stepping; a
//! reload that leaves the mixer alone still keeps it, and a level written
//! live is saved as one value a cold build plays too.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

/// A tone slow enough that its own slope is far below a gain step's.
pub(super) const TONE_HZ: f32 = 110.0;
/// Blocks played before the reload: at least a whole cycle of the tone, so
/// the steepest slope of the tone itself is measured.
pub(super) const BLOCKS_BEFORE: usize = 16;
/// Blocks played after it: the glide, then at least a whole cycle of the
/// tones as the new mixer settles on them.
pub(super) const BLOCKS_AFTER: usize = 24;
/// How much steeper than the tones' own steepest slope, before the reload or
/// once the new mixer has settled, a frame-to-frame change may be: room for
/// the glide's own slope, far below a step.
pub(super) const SLOPE_MARGIN: f32 = 1.5;

/// Two tones and mixer `mix`, whose config is `mixer`, into the dac. The
/// second tone feeds the mixer's second channel when the mixer has one.
pub(super) fn document(mixer: serde_json::Value) -> Invention {
    document_wired(mixer, &[])
}

/// [`document`] with the `extra` connections too.
fn document_wired(mixer: serde_json::Value, extra: &[serde_json::Value]) -> Invention {
    let mut document = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "low", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": TONE_HZ } },
            { "id": "high", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": TONE_HZ * 1.5 } },
            { "id": "mix", "type": "mixer", "config": mixer },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "low", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
            { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
            { "from": "mix", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
        ]
    });
    if document["modules"][2]["config"]["channel_count"].as_u64() > Some(1) {
        document["connections"].as_array_mut().unwrap().push(
            json!({ "from": "high", "from_port": "audio", "to": "mix", "to_port": "audio.1" }),
        );
    }
    let connections = document["connections"].as_array_mut().unwrap();
    connections.extend_from_slice(extra);
    Invention::from_json(&document.to_string()).unwrap()
}

pub(super) fn start(document: Invention) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    start_manual(runtime)
}

/// The largest change between two neighbouring frames.
pub(super) fn steepest(frames: &[f32]) -> f32 {
    frames
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// Plays `from`, reloads to `to`, which must rebuild the mixer, and checks
/// the output across the reload is no steeper than the tones are on their
/// own, before it or once the glide has landed.
fn a_rebuild_does_not_step(from: serde_json::Value, to: serde_json::Value) {
    rebuilds_do_not_step(document(from), &[document(to)]);
}

/// [`a_rebuild_does_not_step`] for reloads to each of `to` in turn, all
/// before the next block, so they install together: the manual backend
/// renders nothing until it is pumped.
fn rebuilds_do_not_step(from: Invention, to: &[Invention]) {
    rebuilds_after_writes_do_not_step(from, &[], to);
}

/// [`rebuilds_do_not_step`], with each of `writes`, a mixer control and a
/// value, written live just before the reloads, in the same block.
fn rebuilds_after_writes_do_not_step(from: Invention, writes: &[(&str, f32)], to: &[Invention]) {
    let (mut running, pump) = start(from);
    let before = pump.render(BLOCKS_BEFORE);
    for (key, value) in writes {
        running.set_control("mix", key, (*value).into()).unwrap();
    }
    for to in to {
        let report = running.reload(to.clone()).expect("the reload applies");
        assert_eq!(report.swapped, ["mix"], "{report:?}");
    }
    let after = pump.render(BLOCKS_AFTER);

    let mut across = before[before.len() - 1..].to_vec();
    across.extend_from_slice(&after);
    let settled = &after[crate::modules::mixer::gain_ramp_frames(SAMPLE_RATE) as usize..];
    let limit = steepest(&before).max(steepest(settled)) * SLOPE_MARGIN;
    assert!(
        steepest(&across) <= limit,
        "steepest {} over {limit}",
        steepest(&across)
    );
}

#[test]
fn a_rebuild_that_changes_the_levels_glides() {
    a_rebuild_does_not_step(
        json!({ "channel_count": 2, "levels": [0.2, 1.0], "master": 1.0 }),
        json!({ "channel_count": 2, "levels": [1.0, 0.2], "master": 0.6 }),
    );
}

#[test]
fn a_rebuild_that_changes_the_pans_glides() {
    a_rebuild_does_not_step(
        json!({ "channel_count": 2, "pans": [-1.0, -1.0] }),
        json!({ "channel_count": 2, "pans": [1.0, 1.0] }),
    );
}

#[test]
fn a_channel_added_by_a_rebuild_fades_in() {
    a_rebuild_does_not_step(json!({ "channel_count": 1 }), json!({ "channel_count": 2 }));
}

#[test]
fn a_channel_dropped_and_restored_before_the_next_block_plays_on() {
    let both = json!({ "channel_count": 2, "levels": [0.0, 1.0] });
    rebuilds_do_not_step(
        document(both.clone()),
        &[document(json!({ "channel_count": 1 })), document(both)],
    );
}

#[test]
fn a_rebuilt_mixer_in_a_feedback_loop_keeps_what_it_fed_back() {
    // The mixer's left output feeds back into its third channel; the
    // rebuilds add an unwired fourth channel and then change its level,
    // installing together in one block.
    let feedback =
        [json!({ "from": "mix", "from_port": "audio_left", "to": "mix", "to_port": "audio.2" })];
    let mixer =
        |channels, fourth| json!({ "channel_count": channels, "levels": [1.0, 0.0, 0.5, fourth] });
    rebuilds_do_not_step(
        document_wired(mixer(3, 0.0), &feedback),
        &[
            document_wired(mixer(4, 0.0), &feedback),
            document_wired(mixer(4, 0.25), &feedback),
        ],
    );
}

#[test]
fn a_reload_that_leaves_the_mixer_alone_keeps_it() {
    let mixer = json!({ "channel_count": 2, "levels": [0.5, 0.5] });
    let (mut running, pump) = start(document(mixer.clone()));
    pump.render(BLOCKS_BEFORE);
    let mut edited = document(mixer);
    edited.modules[1].config["frequency"] = json!(TONE_HZ * 2.0);
    let report = running.reload(edited).expect("the reload applies");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["high.frequency"], "{report:?}");
}

#[test]
fn a_level_written_just_before_a_rebuild_that_adds_a_channel_does_not_step() {
    rebuilds_after_writes_do_not_step(
        document(json!({ "channel_count": 1, "levels": [0.2] })),
        &[("level.0", 1.0), ("master", 0.5)],
        &[document(
            json!({ "channel_count": 2, "levels": [1.0, 0.6], "master": 0.5 }),
        )],
    );
}

/// The retained document as saving writes it and loading reads it back.
fn saved(running: &RunningInvention) -> Invention {
    let json = running.document().unwrap().to_json().unwrap();
    Invention::from_json(&json).unwrap()
}

/// What a cold build of `document`'s mixer holds at `key`.
fn built(document: &Invention, key: &str) -> ControlValue {
    let registry = ModuleRegistry::default();
    let built = registry.build("mixer", SAMPLE_RATE, &document.modules[2].config);
    let surface = built.unwrap().control_surface.unwrap();
    surface.get_control(key).unwrap()
}

#[test]
fn a_level_written_live_over_a_levels_array_saves_as_one_value() {
    let mixer = json!({ "channel_count": 3, "levels": [0.5, 0.5, 0.5] });
    let (mut running, pump) = start(document(mixer));
    pump.render(1);
    running.set_control("mix", "level.2", 0.9.into()).unwrap();
    pump.render(1);

    let saved = saved(&running);
    for (key, value) in [("level.0", 0.5), ("level.1", 0.5), ("level.2", 0.9)] {
        assert_eq!(built(&saved, key), value.into(), "{key}");
        assert_eq!(
            running.get_control("mix", key).unwrap(),
            value.into(),
            "{key}"
        );
    }
    // Loading what was saved keeps the mixer, at most writing the live
    // level again with the value it holds.
    let report = running.reload(saved).expect("the reload applies");
    pump.render(1);
    assert!(report.swapped.is_empty(), "{report:?}");
    assert!(
        report
            .controls_updated
            .iter()
            .all(|key| key == "mix.level.2"),
        "{report:?}"
    );
    assert_eq!(running.get_control("mix", "level.2").unwrap(), 0.9.into());
}

#[test]
fn a_levels_array_reloaded_over_a_live_level_replaces_it() {
    let mixer = json!({ "channel_count": 2, "levels": [0.5, 0.5] });
    let (mut running, pump) = start(document(mixer));
    pump.render(1);
    running.set_control("mix", "level.1", 0.9.into()).unwrap();
    pump.render(1);

    let edited = document(json!({ "channel_count": 2, "levels": [0.25, 0.25] }));
    running.reload(edited.clone()).expect("the reload applies");
    pump.render(BLOCKS_AFTER);
    let saved = saved(&running);
    for key in ["level.0", "level.1"] {
        assert_eq!(
            running.get_control("mix", key).unwrap(),
            0.25.into(),
            "{key}"
        );
        assert_eq!(built(&saved, key), 0.25.into(), "{key}");
    }
}
