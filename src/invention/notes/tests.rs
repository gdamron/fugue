//! A running invention's notes and playhead: a step sequencer on a clock,
//! rendered block by block in place of the audio thread.

use crate::invention::manual_backend::{start_manual, Pump};
use crate::rpc::EditWhen;
use crate::{Invention, InventionBuilder, NoteChange, RunningInvention};

/// 120 BPM at the manual backend's 48 kHz: a beat is 375 blocks of 64.
const BEAT_BLOCKS: usize = 375;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
        { "id": "seq", "type": "step_sequencer", "config": {
            "root_note": 60, "step_count": 4, "gate_length": 0.5,
            "pattern": [{ "note": 0 }, { "note": 4 }, { "note": 7 }, { "note": 12 }]
        } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" }
    ]
}"#;

fn start() -> (RunningInvention, Pump) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    start_manual(runtime)
}

/// Renders `beats` beats, a block at a time.
fn play(pump: &Pump, beats: f64) {
    for _ in 0..(beats * BEAT_BLOCKS as f64) as usize {
        pump.block();
    }
}

fn starts(notes: &[NoteChange]) -> Vec<&NoteChange> {
    notes.iter().filter(|note| note.end.is_none()).collect()
}

#[test]
fn a_sequencer_s_notes_start_on_its_clock_s_beats() {
    let (running, pump) = start();
    play(&pump, 4.25);
    let notes = running.take_notes().notes;
    let started = starts(&notes);
    assert_eq!(started.len(), 5, "beats 0 to 4: {notes:?}");
    for (n, note) in started.iter().enumerate() {
        assert_eq!(note.module_id, "seq");
        assert_eq!(note.note, n as u64);
        assert_eq!(note.voice, 0);
        let beat = note.start.beats.expect("the sequencer has a clock");
        assert!((beat - n as f64).abs() < 1e-3, "note {n} at beat {beat}");
    }
    let pitches: Vec<f32> = started.iter().map(|note| note.pitch.round()).collect();
    assert_eq!(pitches, [60.0, 64.0, 67.0, 72.0, 60.0]);
    // The first four ended, each before the next began; the fifth holds.
    let ended: Vec<_> = notes.iter().filter(|note| note.end.is_some()).collect();
    assert_eq!(ended.len(), 4);
    for note in ended {
        let (start, end) = (note.start.beats.unwrap(), note.end.unwrap().beats.unwrap());
        assert!(end > start && end < start + 1.0, "{note:?}");
    }

    let playhead = running.playhead("seq").expect("the sequencer has a clock");
    assert_eq!(playhead.clock, "clock");
    assert!((playhead.beats - 4.25).abs() < 0.01, "{playhead:?}");
    assert_eq!(running.playhead("nothing"), None, "no such module");
    running.stop();
}

#[test]
fn a_note_held_across_an_edit_is_one_note_and_a_removed_module_s_note_ends() {
    let (running, pump) = start();
    play(&pump, 1.1);
    assert_eq!(starts(&running.take_notes().notes).len(), 2);

    // An edit installs while note 1 holds: it is still that note after.
    running
        .add_module(
            "lfo",
            "oscillator",
            &serde_json::json!({ "frequency": 2.0 }),
        )
        .unwrap();
    play(&pump, 0.5);
    let notes = running.take_notes().notes;
    assert_eq!(notes.len(), 1, "only note 1's end: {notes:?}");
    assert_eq!((notes[0].note, notes[0].end.is_some()), (1, true));

    // Removed while note 2 holds: it ends as the removal installs.
    play(&pump, 0.5);
    assert_eq!(starts(&running.take_notes().notes).len(), 1);
    running.remove_module("seq").unwrap();
    pump.block();
    let notes = running.take_notes().notes;
    assert_eq!(notes.len(), 1, "{notes:?}");
    let end = notes[0].end.expect("ended");
    assert_eq!(notes[0].note, 2);
    assert!((end.beats.unwrap() - 2.1).abs() < 0.01, "{end:?}");
    assert_eq!(running.playhead("seq"), None, "no longer tapped");
    running.stop();
}

#[test]
fn notes_count_on_from_the_next_whole_beat_after_a_reset() {
    let (running, pump) = start();
    play(&pump, 2.5);
    running
        .set_control("clock", "reset", crate::ControlValue::Bool(true))
        .unwrap();
    play(&pump, 0.25);
    let position = running.get_control("clock", "position").unwrap();
    let elapsed = running.get_control("clock", "elapsed").unwrap();
    let (crate::ControlValue::Number(position), crate::ControlValue::Number(elapsed)) =
        (position, elapsed)
    else {
        panic!("numbers");
    };
    // Beats 0, 1 and 2 had begun: the count moves on to 3.
    assert!((position - 0.25).abs() < 0.01, "{position}");
    assert!((elapsed - 3.25).abs() < 0.01, "{elapsed}");
    let notes = running.take_notes().notes;
    let last = starts(&notes).pop().unwrap().start.beats.unwrap();
    assert!(
        (last - 3.0).abs() < 1e-3,
        "the reset's first beat at 3: {last}"
    );
    running.stop();
}

#[test]
fn a_note_held_as_its_module_moves_to_another_clock_ends_and_starts_again_on_it() {
    let (running, pump) = start();
    running
        .add_module("slow", "clock", &serde_json::json!({ "bpm": 60.0 }))
        .unwrap();
    play(&pump, 1.1);
    assert_eq!(starts(&running.take_notes().notes).len(), 2);

    // While note 1 holds, `seq` moves from `clock` (120 BPM, beat 1.1) to
    // `slow` (60 BPM, half a beat in).
    running.disconnect("clock", "beat", "seq", "clock").unwrap();
    running.connect("slow", "beat", "seq", "clock").unwrap();
    pump.block();
    let notes = running.take_notes().notes;
    assert_eq!(notes.len(), 2, "{notes:?}");
    let (ended, restarted) = (&notes[0], &notes[1]);
    assert_eq!((ended.note, restarted.note), (1, 2));
    let end = ended.end.unwrap().beats.unwrap();
    assert!((end - 1.1).abs() < 0.01, "on the old clock: {end}");
    let start = restarted.start.beats.unwrap();
    assert!((start - 0.55).abs() < 0.01, "on the new one: {start}");
    assert_eq!(running.playhead("seq").unwrap().clock, "slow");
    running.stop();
}

#[test]
fn a_timed_edit_leaves_the_playhead_on_the_installed_clock_until_it_lands() {
    let (running, pump) = start();
    play(&pump, 0.5);
    // Replace the sequencer with an oscillator, a hundred beats from now.
    let when = EditWhen::AfterBeats {
        beats: 100.0,
        clock: None,
    };
    let config = serde_json::json!({ "frequency": 220.0 });
    running
        .swap_module_at("seq", "oscillator", &config, false, &when, None)
        .unwrap();
    play(&pump, 1.0);
    let playhead = running
        .playhead("seq")
        .expect("still the installed sequencer");
    assert!((playhead.beats - 1.5).abs() < 0.01, "{playhead:?}");
    assert_eq!(
        starts(&running.take_notes().notes).len(),
        2,
        "still playing"
    );
    running.stop();
}
