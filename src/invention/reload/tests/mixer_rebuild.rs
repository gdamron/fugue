//! A reload that rebuilds a mixer while it plays glides from what the old
//! mixer was playing to the new one's levels, rather than stepping
//! (FUG-375); a reload that leaves the mixer alone still keeps it.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

/// A tone slow enough that its own slope is far below a gain step's.
const TONE_HZ: f32 = 110.0;
/// Blocks played before the reload: at least a whole cycle of the tone, so
/// the steepest slope of the tone itself is measured.
const BLOCKS_BEFORE: usize = 16;
/// Blocks played after it: the glide, then at least a whole cycle of the
/// tones as the new mixer settles on them.
const BLOCKS_AFTER: usize = 24;
/// How much steeper than the tones' own steepest slope, before the reload or
/// once the new mixer has settled, a frame-to-frame change may be: room for
/// the glide's own slope, far below a step.
const SLOPE_MARGIN: f32 = 1.5;

/// Two tones and mixer `mix`, whose config is `mixer`, into the dac. The
/// second tone feeds the mixer's second channel when the mixer has one.
fn document(mixer: serde_json::Value) -> Invention {
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
    Invention::from_json(&document.to_string()).unwrap()
}

fn start(mixer: serde_json::Value) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(document(mixer))
        .unwrap();
    start_manual(runtime)
}

/// The largest change between two neighbouring frames.
fn steepest(frames: &[f32]) -> f32 {
    frames
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// Plays `from`, reloads to `to`, which must rebuild the mixer, and checks
/// the output across the reload is no steeper than the tones are on their
/// own, before it or once the glide has landed.
fn a_rebuild_does_not_step(from: serde_json::Value, to: serde_json::Value) {
    let (mut running, pump) = start(from);
    let before = pump.render(BLOCKS_BEFORE);
    let report = running.reload(document(to)).expect("the reload applies");
    assert_eq!(report.swapped, ["mix"], "{report:?}");
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
fn a_reload_that_leaves_the_mixer_alone_keeps_it() {
    let mixer = json!({ "channel_count": 2, "levels": [0.5, 0.5] });
    let (mut running, pump) = start(mixer.clone());
    pump.render(BLOCKS_BEFORE);
    let mut edited = document(mixer);
    edited.modules[1].config["frequency"] = json!(TONE_HZ * 2.0);
    let report = running.reload(edited).expect("the reload applies");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["high.frequency"], "{report:?}");
}
