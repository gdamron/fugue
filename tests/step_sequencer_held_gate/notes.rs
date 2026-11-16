//! The played-note report, read from a running invention with
//! `RunningInvention::take_notes`, shows a note and its held step as one
//! note, not a second one at the held step's edge.

use std::time::{Duration, Instant};

use fugue::{Invention, InventionBuilder, NoteChange};
use serde_json::json;

use super::rig::{clock, dac, document, sequencer, RATE, STEP};
use super::shared::NullAudioBackend;

/// Plays `pattern` live until `count` of its notes have ended, and returns
/// every note change the sequencer reported by then.
fn played_notes(pattern: serde_json::Value, count: usize) -> Vec<NoteChange> {
    let json = document(
        vec![clock(), sequencer(pattern, json!({})), dac()],
        &[["clock", "beat", "seq", "clock"]],
    );
    let invention = Invention::from_json(&json).unwrap();
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(RATE))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut changes = Vec::new();
    while changes
        .iter()
        .filter(|note: &&NoteChange| note.end.is_some())
        .count()
        < count
    {
        assert!(
            Instant::now() < deadline,
            "notes stopped ending: {changes:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
        let taken = running.take_notes();
        assert_eq!(taken.missed, 0, "no note change was dropped");
        changes.extend(
            taken
                .notes
                .into_iter()
                .filter(|note| note.module_id == "seq"),
        );
    }
    running.stop();
    changes
}

#[test]
fn a_note_and_its_held_step_report_as_one_note() {
    let pattern = json!([{ "note": null }, { "note": 0 }, { "held": true }, { "note": null }]);
    let changes = played_notes(pattern, 3);
    let ended: Vec<&NoteChange> = changes.iter().filter(|n| n.end.is_some()).collect();
    for note in &ended {
        assert_eq!(note.pitch.round(), 60.0, "{note:?}");
        let length = note.end.unwrap().frame - note.start.frame;
        assert!(
            length as usize > 2 * STEP - STEP / 4,
            "note {} lasted {length} frames: the held step should extend it to about two \
             steps ({}), not start a note of its own (ended: {ended:#?})",
            note.note,
            2 * STEP
        );
    }
    // One start per pass: consecutive notes start a whole pattern (four
    // steps) apart, never a step apart.
    let starts: Vec<u64> = changes
        .iter()
        .filter(|n| n.end.is_none())
        .map(|n| n.start.frame)
        .collect();
    for pair in starts.windows(2) {
        assert!(
            (pair[1] - pair[0]) as usize > 3 * STEP,
            "notes started {} frames apart: {starts:?}",
            pair[1] - pair[0]
        );
    }
}
