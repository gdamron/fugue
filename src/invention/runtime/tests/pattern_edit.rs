//! A step sequencer's pattern edited while it plays, as a piano roll edits
//! it: one authored `apply_edits` write of the whole pattern. The module is
//! not rebuilt, its position is kept, and the edit is heard from the next
//! time the sequencer reaches the step, with nothing else in the output
//! changing.

use crate::invention::manual_backend::start_manual;
use crate::rpc::StructuralEdit;
use crate::{Invention, InventionBuilder};

const DOC: &str = include_str!("../../../../examples/step_sequencer.json");

/// 64-frame blocks in one step: the example's clock beats at 120 bpm.
const STEP_BLOCKS: usize = 375;

/// The example's pattern with step `step` set to `offset`.
fn edited(step: usize, offset: i64) -> serde_json::Value {
    let document: serde_json::Value = serde_json::from_str(DOC).unwrap();
    let mut pattern = document["modules"][1]["config"]["pattern"].clone();
    pattern[step]["note"] = offset.into();
    pattern
}

/// The example with `pattern` as loaded.
fn loaded_with(pattern: &serde_json::Value) -> String {
    let mut document: serde_json::Value = serde_json::from_str(DOC).unwrap();
    document["modules"][1]["config"]["pattern"] = pattern.clone();
    document.to_string()
}

/// Plays `doc` for `blocks`, making `edit` (a pattern) after `at` blocks.
fn session(doc: &str, blocks: usize, edit: Option<(usize, &serde_json::Value)>) -> Vec<f32> {
    let invention = Invention::from_json(doc).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let Some((at, pattern)) = edit else {
        let played = pump.render(blocks);
        running.stop();
        return played;
    };
    let mut played = pump.render(at);
    let text = pattern.to_string();
    let edit = StructuralEdit::SetControl {
        module_id: "seq".into(),
        key: "pattern".into(),
        value: text.clone().into(),
    };
    let report = running.apply_edits(&[edit]).expect("the edit commits");
    assert!(
        report.rebuilt.is_empty() && report.added.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.untouched, 6, "every module keeps its instance");
    let document = running.document().unwrap();
    let seq = document.modules.iter().find(|module| module.id == "seq");
    let authored: serde_json::Value =
        serde_json::from_str(seq.unwrap().config["pattern"].as_str().unwrap()).unwrap();
    assert_eq!(&authored, pattern, "the document holds the edited pattern");
    played.extend(pump.render(blocks - at));
    running.stop();
    played
}

fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
}

/// An edit lands this far into a step: inside the step's gate, so a note is
/// sounding when the pattern is swapped.
const INTO_STEP: usize = STEP_BLOCKS / 10;

/// The piece is audible around block `at`: an edit there would be heard if
/// it disturbed the voice.
fn assert_sounding(output: &[f32], at: usize) {
    let around = &output[at * 64..(at + 8) * 64];
    let peak = around.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 1e-3, "silent at the edit (peak {peak})");
}

#[test]
fn an_edit_ahead_of_the_playhead_sounds_as_if_the_pattern_had_always_held_it() {
    let pattern = edited(6, 12);
    let blocks = 18 * STEP_BLOCKS;
    let at = 4 * STEP_BLOCKS + INTO_STEP;
    let live = session(DOC, blocks, Some((at, &pattern)));
    let as_loaded = session(&loaded_with(&pattern), blocks, None);
    let unedited = session(DOC, blocks, None);
    assert_sounding(&unedited, at);
    // The same output, sample for sample, as the edited pattern played from
    // its start: no click, no restart and no jump in position.
    assert_eq!(first_difference(&live, &as_loaded), None);
    // And the same as the old pattern until step 7 sounds.
    let changed = first_difference(&live, &unedited).expect("the edit is heard");
    assert!(
        changed >= 6 * STEP_BLOCKS * 64,
        "changed at sample {changed}"
    );
    assert!(
        changed < 7 * STEP_BLOCKS * 64,
        "changed at sample {changed}"
    );
}

#[test]
fn an_edit_to_the_sounding_step_is_heard_the_next_time_round() {
    // Step 3 is sounding as the edit lands; it plays out as it began.
    let pattern = edited(2, 7);
    let blocks = 20 * STEP_BLOCKS;
    let at = 2 * STEP_BLOCKS + INTO_STEP;
    let live = session(DOC, blocks, Some((at, &pattern)));
    let unedited = session(DOC, blocks, None);
    assert_sounding(&unedited, at);
    let changed = first_difference(&live, &unedited).expect("the edit is heard");
    assert!(
        changed >= 18 * STEP_BLOCKS * 64,
        "changed at sample {changed}"
    );
    assert!(
        changed < 19 * STEP_BLOCKS * 64,
        "changed at sample {changed}"
    );
}

#[test]
fn a_pattern_read_from_a_score_file_is_replaced_in_the_document_only() {
    let mut document: serde_json::Value = serde_json::from_str(DOC).unwrap();
    let pattern = document["modules"][1]["config"]["pattern"].clone();
    let dir = tempfile::tempdir().unwrap();
    let score = dir.path().join("score.json");
    let written = serde_json::json!({ "cells": [pattern] }).to_string();
    std::fs::write(&score, &written).unwrap();
    let modified = std::fs::metadata(&score).unwrap().modified().unwrap();
    document["assets"] = serde_json::json!({ "score": { "path": score } });
    document["modules"][1]["config"]["pattern"] =
        serde_json::json!({ "$asset": "score", "path": "/cells/0" });
    let invention = Invention::from_json(&document.to_string()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    pump.render(4);
    let new = edited(0, 5);
    let edit = StructuralEdit::SetControl {
        module_id: "seq".into(),
        key: "pattern".into(),
        value: new.to_string().into(),
    };
    running.apply_edits(&[edit]).expect("the edit commits");
    let saved = running.document().unwrap();
    let seq = saved
        .modules
        .iter()
        .find(|module| module.id == "seq")
        .unwrap();
    let authored: serde_json::Value =
        serde_json::from_str(seq.config["pattern"].as_str().unwrap()).unwrap();
    assert_eq!(authored, new, "the module holds the edited pattern");
    assert!(
        saved.assets.contains_key("score"),
        "the score stays declared"
    );
    // What a save would write holds the edit too.
    let to_save = running.document_to_save().expect("a document to save");
    let seq = to_save.modules.iter().find(|module| module.id == "seq");
    let saved_pattern: serde_json::Value =
        serde_json::from_str(seq.unwrap().config["pattern"].as_str().unwrap()).unwrap();
    assert_eq!(saved_pattern, new);
    assert_eq!(
        std::fs::read_to_string(&score).unwrap(),
        written,
        "never written"
    );
    assert_eq!(
        std::fs::metadata(&score).unwrap().modified().unwrap(),
        modified
    );
    running.stop();
}

#[test]
fn a_pattern_the_sequencer_cannot_hold_is_refused_and_nothing_changes() {
    let mut too_long = vec![serde_json::json!({ "note": 0 }); 65];
    too_long[0] = serde_json::json!(0);
    let refused = [
        "not json".to_string(),
        r#"{"note": 3}"#.to_string(),
        serde_json::Value::from(too_long).to_string(),
        r#"[{"note": 3, "pitch": 60}]"#.to_string(),
        r#"[{"note": 300}]"#.to_string(),
        r#"[{"note": 0, "gate_length": "long"}]"#.to_string(),
    ];
    let blocks = 18 * STEP_BLOCKS;
    let at = 4 * STEP_BLOCKS + INTO_STEP;
    let invention = Invention::from_json(DOC).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let mut played = pump.render(at);
    let before = serde_json::to_value(running.document().unwrap()).unwrap();
    for value in &refused {
        let edit = StructuralEdit::SetControl {
            module_id: "seq".into(),
            key: "pattern".into(),
            value: value.clone().into(),
        };
        let error = running
            .apply_edits(&[edit])
            .expect_err(&format!("{value} is refused"));
        assert!(!error.message.is_empty());
        let after = serde_json::to_value(running.document().unwrap()).unwrap();
        assert_eq!(after, before, "{value} changed the document");
    }
    played.extend(pump.render(blocks - at));
    running.stop();
    assert_eq!(first_difference(&played, &session(DOC, blocks, None)), None);
}
