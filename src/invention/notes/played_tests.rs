//! Played notes fed as words: paired as the live feed pairs them, paged as
//! the daemon pages them, and words it cannot read refused whole.

use super::{PlayedNotes, EDGE_WORDS, HEADER_WORDS, TAP_WORDS};

const RATE: u32 = 48_000;

/// Words for run `run`: `taps` as (key, clock key, beats), then `edges` as
/// (key, voice, on, frame, beats, pitch frequency).
fn words(run: u64, keys: usize, taps: &[(f64, f64, f64)], edges: &[[f64; 6]]) -> Vec<f64> {
    let mut words = vec![
        run as f64,
        0.0,
        keys as f64,
        taps.len() as f64,
        edges.len() as f64,
    ];
    for &(key, clock, beats) in taps {
        words.extend([key, clock, beats]);
    }
    for &[key, voice, on, frame, beats, frequency] in edges {
        let velocity = if on == 1.0 { 0.5 } else { 0.0 };
        words.extend([key, voice, on, frame, beats, 1.0, frequency, velocity, 0.0]);
    }
    assert_eq!(
        words.len(),
        HEADER_WORDS + taps.len() * TAP_WORDS + edges.len() * EDGE_WORDS
    );
    words
}

fn played() -> PlayedNotes {
    let mut played = PlayedNotes::starting_at(1_000);
    played.set_names(vec!["seq".to_string(), "clock".to_string()]);
    played
}

#[test]
fn before_anything_renders_there_are_no_notes_and_the_playhead_is_at_the_start() {
    let mut played = played();
    played
        .feed(&words(1, 2, &[(0.0, 1.0, f64::NAN)], &[]))
        .unwrap();
    let page = played.page("seq", None, RATE);
    assert!(page.notes.is_empty() && !page.resync_required);
    assert_eq!(
        (page.clock.as_deref(), page.playhead),
        (Some("clock"), Some(0.0))
    );
}

#[test]
fn edges_pair_into_notes_as_they_now_stand() {
    let mut played = played();
    let tap = [(0.0, 1.0, 1.5)];
    played
        .feed(&words(1, 2, &tap, &[[0.0, 0.0, 1.0, 24_000.0, 1.0, 440.0]]))
        .unwrap();
    let first = played.page("seq", None, RATE);
    assert_eq!(first.playhead, Some(1.5));
    let held = &first.notes[0];
    assert_eq!((held.id.as_str(), held.note.pitch), ("0", 69));
    assert_eq!((held.note.end_beats, held.note.end_seconds), (None, None));
    assert_eq!(held.note.velocity, 0.5);

    played
        .feed(&words(1, 2, &tap, &[[0.0, 0.0, 0.0, 36_000.0, 1.5, 0.0]]))
        .unwrap();
    let next = played.page("seq", Some(first.latest_seq), RATE);
    assert!(!next.resync_required);
    let ended = &next.notes[0].note;
    assert_eq!(next.notes[0].id, "0", "the same note, now ended");
    assert_eq!((ended.start_beats, ended.end_beats), (Some(1.0), Some(1.5)));
    assert_eq!((ended.start_seconds, ended.end_seconds), (0.5, Some(0.75)));
    assert_eq!(ended.clock.as_deref(), Some("clock"));
}

#[test]
fn a_new_run_starts_over_and_resyncs_every_cursor() {
    let mut played = played();
    let on = [0.0, 0.0, 1.0, 0.0, 0.0, 440.0];
    played.feed(&words(1, 2, &[], &[on])).unwrap();
    let cursor = played.page("seq", None, RATE).latest_seq;
    // The next piece: its note 0 is not the last one's, and the note held
    // in the last run is forgotten rather than ended.
    played.feed(&words(2, 2, &[], &[on])).unwrap();
    let page = played.page("seq", Some(cursor), RATE);
    assert!(page.resync_required);
    assert_eq!(page.notes.len(), 1);
    assert_eq!(page.notes[0].note.end_seconds, None);
}

#[test]
fn words_it_cannot_read_are_refused_with_nothing_taken() {
    let mut played = played();
    let on = [0.0, 0.0, 1.0, 0.0, 0.0, 440.0];
    assert!(played.feed(&[1.0, 0.0]).unwrap_err().contains("header"));
    let mut short = words(1, 2, &[], &[on]);
    short.pop();
    assert!(played.feed(&short).unwrap_err().contains("cut short"));
    let unnamed = words(1, 3, &[], &[on]);
    assert_eq!(PlayedNotes::keys_in(&unnamed), 3);
    assert!(played
        .feed(&unnamed)
        .unwrap_err()
        .contains("set the names first"));
    assert!(played.page("seq", None, RATE).notes.is_empty());
}
