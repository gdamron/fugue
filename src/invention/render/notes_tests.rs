//! An offline render that follows its notes: what it plays, read paired or
//! as words a page pairs, through edits and loads, sounding exactly as it
//! does unfollowed.

use super::RenderEngine;
use crate::alloc_counter::allocator_events;
use crate::invention::notes::{NoteHistory, PlayedNotes, NOTE_WORDS_LEN};

const RATE: u32 = 48_000;

/// Frames a player renders between two drains.
const CHUNK: usize = 2048;

/// A step sequencer on the beat of a 120 bpm clock: C, rest, G, rest.
fn bass(root: u8) -> String {
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "seq", "type": "step_sequencer", "config": {
                "root_note": root, "step_count": 4, "gate_length": 0.5,
                "pattern": [{ "note": 0 }, { "note": null }, { "note": 7 }, { "note": null }]
            }},
            { "id": "osc", "type": "oscillator" },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" },
            { "from": "seq", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    })
    .to_string()
}

/// Renders `seconds` in player-sized chunks, feeding each chunk's words to
/// `played` when given one. Returns what it rendered.
fn render(engine: &mut RenderEngine, seconds: f64, played: Option<&mut PlayedNotes>) -> Vec<f32> {
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut words = vec![0.0; NOTE_WORDS_LEN];
    let mut played = played;
    let mut out = Vec::with_capacity(frames * 2);
    let mut chunk = vec![0.0f32; CHUNK * 2];
    let mut done = 0;
    while done < frames {
        let n = CHUNK.min(frames - done);
        engine.render_interleaved(&mut chunk[..n * 2]).unwrap();
        out.extend_from_slice(&chunk[..n * 2]);
        done += n;
        if let Some(played) = played.as_deref_mut() {
            let written = engine.take_note_words(&mut words);
            if PlayedNotes::keys_in(&words[..written]) > played.names_known() {
                played.set_names(engine.note_names());
            }
            played.feed(&words[..written]).unwrap();
        }
    }
    out
}

fn followed(document: &str) -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(document).unwrap();
    engine.follow_notes();
    engine
}

#[test]
fn following_notes_changes_nothing_it_renders() {
    let mut plain = RenderEngine::new(RATE);
    plain.load_json(&bass(48)).unwrap();
    let mut engine = followed(&bass(48));
    let mut played = PlayedNotes::starting_at(1);
    let heard = render(&mut plain, 3.5, None);
    assert_eq!(render(&mut engine, 3.5, Some(&mut played)), heard);
    assert_eq!(played.page("seq", None, RATE).notes.len(), 4);
}

#[test]
fn words_fed_to_a_page_list_what_the_paired_feed_lists() {
    let mut engine = followed(&bass(48));
    let mut played = PlayedNotes::starting_at(1_000);
    // A note every two beats, a beat every half second.
    render(&mut engine, 3.5, Some(&mut played));
    let page = played.page("seq", None, RATE);
    let pitches: Vec<u8> = page.notes.iter().map(|note| note.note.pitch).collect();
    assert_eq!(pitches, [48, 55, 48, 55]);
    for (note, beat) in page.notes.iter().zip([0.0, 2.0, 4.0, 6.0]) {
        let start = note.note.start_beats.unwrap();
        assert!((start - beat).abs() < 0.01, "{start}");
        assert!(note.note.end_seconds.is_some());
    }
    assert!((page.playhead.unwrap() - 7.0).abs() < 0.01);

    // The same render read paired, into the daemon's history, pages alike.
    let mut paired = followed(&bass(48));
    let mut history = NoteHistory::starting_at(1_000);
    let mut chunk = vec![0.0f32; CHUNK * 2];
    let mut left = (3.5 * f64::from(RATE)) as usize;
    while left > 0 {
        let n = CHUNK.min(left);
        paired.render_interleaved(&mut chunk[..n * 2]).unwrap();
        history.record(1, paired.take_notes());
        left -= n;
    }
    let daemon = history.page("seq", None, None, RATE);
    assert_eq!(daemon.notes, page.notes);
}

#[test]
fn a_module_an_edit_adds_is_followed_and_a_note_held_across_it_stays_one() {
    let mut engine = followed(&bass(48));
    let mut played = PlayedNotes::starting_at(1);
    // Into the first note, held from beat 0 to 0.5.
    render(&mut engine, 0.1, Some(&mut played));
    let config = serde_json::json!({
        "root_note": 72, "step_count": 2, "gate_length": 0.5,
        "pattern": [{ "note": 0 }, { "note": null }]
    });
    engine
        .add_module("lead", "step_sequencer", &config)
        .unwrap();
    engine.connect("clock", "beat", "lead", "clock").unwrap();
    render(&mut engine, 2.0, Some(&mut played));
    let bass_notes = played.page("seq", None, RATE).notes;
    assert_eq!(bass_notes[0].id, "0");
    let first = &bass_notes[0].note;
    assert!(first.start_seconds < 0.01 && first.end_seconds.unwrap() > 0.2);
    let lead = played.page("lead", None, RATE);
    assert!(
        !lead.notes.is_empty(),
        "the added module's notes are followed"
    );
    assert!(lead.notes.iter().all(|note| note.note.pitch == 72));
}

#[test]
fn a_new_load_is_a_new_run_and_resyncs_its_readers() {
    let mut engine = followed(&bass(48));
    let mut played = PlayedNotes::starting_at(1);
    render(&mut engine, 1.0, Some(&mut played));
    let cursor = played.page("seq", None, RATE).latest_seq;
    engine.load_json(&bass(60)).unwrap();
    render(&mut engine, 1.0, Some(&mut played));
    let page = played.page("seq", Some(cursor), RATE);
    assert!(page.resync_required);
    assert!(page
        .notes
        .iter()
        .all(|note| [60, 67].contains(&note.note.pitch)));
}

#[test]
fn names_fetched_for_one_piece_still_hold_for_the_next() {
    let mut engine = followed(&bass(48));
    let mut played = PlayedNotes::starting_at(1);
    render(&mut engine, 1.0, Some(&mut played));
    // The next piece's modules have other ids, as many of them.
    let mut renamed = bass(60);
    for (from, to) in [("clock", "tempo"), ("seq", "melody")] {
        for field in ["id", "from", "to"] {
            renamed = renamed.replace(
                &format!("\"{field}\":\"{from}\""),
                &format!("\"{field}\":\"{to}\""),
            );
        }
    }
    assert!(renamed.contains("\"id\":\"melody\"") && renamed.contains("\"to_port\":\"clock\""));
    engine.load_json(&renamed).unwrap();
    render(&mut engine, 1.0, Some(&mut played));
    let melody = played.page("melody", None, RATE);
    assert!(!melody.notes.is_empty());
    assert!(
        melody
            .notes
            .iter()
            .all(|note| [60, 67].contains(&note.note.pitch)
                && note.note.clock.as_deref() == Some("tempo")),
        "{:?}",
        melody.notes
    );
    let seq = played.page("seq", None, RATE);
    assert!(seq
        .notes
        .iter()
        .all(|note| [48, 55].contains(&note.note.pitch)));
}

#[test]
fn draining_neither_allocates_nor_frees() {
    let mut engine = followed(&bass(48));
    let mut words = vec![0.0; NOTE_WORDS_LEN];
    let mut chunk = vec![0.0f32; CHUNK * 2];
    let mut written = 0;
    // Allocation the render itself may do is not the drain's: count only
    // the drains, each after a render that ends a note or starts one.
    for _ in 0..24 {
        engine.render_interleaved(&mut chunk).unwrap();
        let (n, allocs, frees) = allocator_events(|| engine.take_note_words(&mut words));
        assert_eq!((allocs, frees), (0, 0));
        written += n;
    }
    assert!(written > 0);
}

#[test]
fn nothing_is_drained_unfollowed_or_unloaded() {
    let mut words = vec![0.0; NOTE_WORDS_LEN];
    let mut engine = RenderEngine::new(RATE);
    engine.follow_notes();
    assert_eq!(engine.take_note_words(&mut words), 0, "nothing loaded");
    let mut plain = RenderEngine::new(RATE);
    plain.load_json(&bass(48)).unwrap();
    render(&mut plain, 1.0, None);
    assert_eq!(plain.take_note_words(&mut words), 0, "not following");
    assert!(plain.take_notes().notes.is_empty());
    assert!(plain.note_names().is_empty());
}
