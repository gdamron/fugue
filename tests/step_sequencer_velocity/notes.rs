//! Played-note reports carry the step's velocity, read from a running
//! invention with `RunningInvention::take_notes`. The sequencer's
//! velocity output is connected to nothing: reports read it regardless.

use std::time::{Duration, Instant};

use fugue::{Invention, InventionBuilder, NoteChange};
use serde_json::json;

use super::rig::{clock, dac, document, sequencer, RATE, TOLERANCE};
use super::shared::NullAudioBackend;

/// Plays `pattern` live until `count` notes have started, and returns them.
fn started_notes(pattern: serde_json::Value, count: usize) -> Vec<NoteChange> {
    let json = document(
        vec![
            clock(),
            sequencer(json!({
                "root_note": 60, "step_count": pattern.as_array().unwrap().len(),
                "gate_length": 0.5, "pattern": pattern
            })),
            dac(),
        ],
        &[["clock", "beat", "seq", "clock"]],
    );
    let invention = Invention::from_json(&json).unwrap();
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(RATE))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut started = Vec::new();
    while started.len() < count {
        assert!(
            Instant::now() < deadline,
            "only {} notes started",
            started.len()
        );
        std::thread::sleep(Duration::from_millis(20));
        let changes = running.take_notes();
        assert_eq!(changes.missed, 0, "no note change was dropped");
        started.extend(
            changes
                .notes
                .into_iter()
                .filter(|note| note.module_id == "seq" && note.end.is_none()),
        );
    }
    running.stop();
    started
}

fn assert_velocities(notes: &[NoteChange], by_pitch: &[(f32, f32)]) {
    for note in notes {
        let pitch = note.pitch.round();
        let &(_, want) = by_pitch
            .iter()
            .find(|(p, _)| *p == pitch)
            .unwrap_or_else(|| panic!("an unexpected pitch: {note:?}"));
        assert!(
            (note.velocity - want).abs() <= TOLERANCE,
            "note {} at pitch {pitch} reports velocity {}, want {want}",
            note.note,
            note.velocity
        );
    }
}

#[test]
fn a_played_note_reports_its_step_s_velocity_and_full_without_one() {
    let notes = started_notes(
        json!([
            { "note": 0, "velocity": 0.3 },
            { "note": 2, "velocity": 0.6 },
            { "note": 4 },
            { "note": 7, "velocity": 0.85 }
        ]),
        9,
    );
    assert_velocities(
        &notes,
        &[(60.0, 0.3), (62.0, 0.6), (64.0, 1.0), (67.0, 0.85)],
    );
}

#[test]
fn a_grace_note_reports_eight_tenths_of_its_step_s_velocity() {
    let notes = started_notes(
        json!([
            { "note": null },
            { "note": 7, "velocity": 0.5, "grace": [5] }
        ]),
        6,
    );
    let pitches: Vec<f32> = notes.iter().map(|note| note.pitch.round()).collect();
    assert!(
        pitches.contains(&65.0) && pitches.contains(&67.0),
        "{pitches:?}"
    );
    assert_velocities(&notes, &[(65.0, 0.8 * 0.5), (67.0, 0.5)]);
}
