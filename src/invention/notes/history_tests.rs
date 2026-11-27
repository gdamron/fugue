//! The note history: a module's notes read by cursor, bounded and pruned,
//! and a reader that resumes from its cursor sees every change once.

use super::{NoteHistory, NOTES_PER_MODULE};
use crate::invention::{NoteChange, NoteChanges, NoteTime, Playhead};
use crate::rpc::NotePage;

const RATE: u32 = 48_000;

fn at(beats: f64) -> NoteTime {
    NoteTime {
        frame: (beats * 24_000.0) as u64,
        beats: Some(beats),
    }
}

/// Note `n` of `module` on voice `voice` at MIDI `pitch`, from `start`
/// beats, ended at `end` or held.
fn note(module: &str, n: u64, voice: u32, pitch: f32, start: f64, end: Option<f64>) -> NoteChange {
    NoteChange {
        module_id: module.to_string(),
        note: n,
        voice,
        pitch,
        velocity: 0.8,
        clock: Some("clock".to_string()),
        start: at(start),
        end: end.map(at),
    }
}

fn played(notes: Vec<NoteChange>) -> NoteChanges {
    NoteChanges { notes, missed: 0 }
}

fn playhead(beats: f64) -> Option<Playhead> {
    on("clock", beats)
}

fn on(clock: &str, beats: f64) -> Option<Playhead> {
    Some(Playhead {
        clock: clock.to_string(),
        beats,
    })
}

fn poll(history: &NoteHistory, module: &str, after: Option<u64>) -> NotePage {
    history.page(module, after, playhead(0.0), RATE)
}

/// The ids and ends a page carries, in order.
fn ids(page: &NotePage) -> Vec<(String, Option<f64>)> {
    page.notes
        .iter()
        .map(|note| (note.id.clone(), note.note.end_beats))
        .collect()
}

#[test]
fn a_page_carries_each_note_as_it_stands_and_overlapping_voices_apart() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(
        1,
        played(vec![
            note("seq", 0, 0, 69.0, 1.0, None),
            note("seq", 1, 1, 72.4, 2.0, None),
            note("seq", 1, 1, 72.4, 2.0, Some(2.5)),
            note("other", 0, 0, 40.0, 0.0, None),
        ]),
    );
    let page = history.page("seq", None, playhead(2.75), RATE);
    assert!(!page.resync_required);
    assert_eq!(page.clock.as_deref(), Some("clock"));
    assert_eq!(page.playhead, Some(2.75));
    // Note 1's start and end are one entry, as it now stands.
    assert_eq!(ids(&page), [("0".into(), None), ("1".into(), Some(2.5))]);
    let (a, c) = (&page.notes[0], &page.notes[1]);
    assert_eq!(
        (a.note.voice.as_str(), a.note.pitch, a.note.start_beats),
        ("0", 69, Some(1.0))
    );
    assert_eq!(
        (c.note.voice.as_str(), c.note.pitch),
        ("1", 72),
        "to the nearest note"
    );
    // Every note carries its module, clock, velocity and seconds too.
    assert_eq!(a.note.module_id, "seq");
    assert_eq!(a.note.clock.as_deref(), Some("clock"));
    assert_eq!((a.note.velocity, a.note.start_seconds), (0.8, 0.5));
    assert_eq!(
        (c.note.end_beats, c.note.end_seconds),
        (Some(2.5), Some(1.25))
    );
    assert!(a.seq < c.seq);
    assert_eq!(
        page.latest_seq,
        c.seq + 1,
        "the other module's note came after"
    );
}

#[test]
fn a_reader_resuming_from_its_cursor_sees_every_change_once() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(1, played(vec![note("seq", 0, 0, 60.0, 0.0, None)]));
    let first = poll(&history, "seq", None);
    assert_eq!(ids(&first), [("0".into(), None)]);

    // Held across the poll, then ended, then another starts.
    history.record(
        1,
        played(vec![
            note("seq", 0, 0, 60.0, 0.0, Some(0.5)),
            note("seq", 1, 0, 64.0, 1.0, None),
        ]),
    );
    let second = poll(&history, "seq", Some(first.latest_seq));
    assert!(!second.resync_required);
    assert_eq!(ids(&second), [("0".into(), Some(0.5)), ("1".into(), None)]);

    // Reconnecting with the same cursor reads the same changes; nothing new
    // after the latest.
    assert_eq!(poll(&history, "seq", Some(first.latest_seq)), second);
    let idle = poll(&history, "seq", Some(second.latest_seq));
    assert!(idle.notes.is_empty() && !idle.resync_required);
    assert_eq!(idle.latest_seq, second.latest_seq);
}

#[test]
fn the_history_keeps_its_cap_pruning_the_oldest_finished_notes_first() {
    let mut history = NoteHistory::starting_at(1_000);
    // One note held from the start, then far more finished ones than kept.
    history.record(1, played(vec![note("seq", 0, 1, 48.0, 0.0, None)]));
    let early = poll(&history, "seq", None).latest_seq;
    let finished = (1..=NOTES_PER_MODULE as u64 + 100)
        .map(|n| note("seq", n, 0, 60.0, n as f64, Some(n as f64 + 0.5)));
    history.record(1, played(finished.collect()));
    let page = poll(&history, "seq", None);
    assert_eq!(page.notes.len(), NOTES_PER_MODULE);
    let kept: Vec<&str> = page.notes.iter().map(|note| note.id.as_str()).collect();
    assert_eq!(kept[0], "0", "the held note stays");
    assert_eq!(kept[1], "102", "notes 1 to 101 were pruned");

    // A reader that last saw the held note missed pruned changes: it is
    // told so and handed everything kept.
    let behind = poll(&history, "seq", Some(early));
    assert!(behind.resync_required);
    assert_eq!(behind.notes, page.notes);
    // One that saw everything resumes.
    let current = poll(&history, "seq", Some(page.latest_seq));
    assert!(!current.resync_required && current.notes.is_empty());
}

#[test]
fn a_new_run_or_a_missed_change_makes_every_cursor_resync() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(1, played(vec![note("seq", 0, 0, 60.0, 0.0, None)]));
    let cursor = poll(&history, "seq", None).latest_seq;

    // A different invention starts: its note 0 is not the last one's.
    history.record(2, played(vec![note("seq", 0, 0, 67.0, 0.0, None)]));
    let page = poll(&history, "seq", Some(cursor));
    assert!(page.resync_required);
    assert_eq!(page.notes.len(), 1);
    assert_eq!(page.notes[0].note.pitch, 67);

    let cursor = page.latest_seq;
    history.record(
        2,
        NoteChanges {
            notes: Vec::new(),
            missed: 3,
        },
    );
    assert!(poll(&history, "seq", Some(cursor)).resync_required);

    // So does a cursor this daemon never issued.
    let ahead = poll(&history, "seq", Some(page.latest_seq + 10));
    assert!(ahead.resync_required);
    assert!(
        poll(&history, "seq", Some(1)).resync_required,
        "an earlier daemon's"
    );
}

#[test]
fn a_cursor_from_before_the_history_started_resyncs() {
    let history = NoteHistory::starting_at(1_000);
    let page = poll(&history, "seq", None);
    assert!(page.notes.is_empty() && !page.resync_required);
    assert_eq!(page.latest_seq, 999);
    assert!(poll(&history, "seq", Some(998)).resync_required);
    assert!(!poll(&history, "seq", Some(999)).resync_required);
    assert_eq!(NoteHistory::starting_at(0).latest_seq(), 0);
}

#[test]
fn without_a_clock_notes_are_timed_in_seconds_alone() {
    let mut history = NoteHistory::starting_at(1_000);
    let mut change = note("drone", 0, 0, 57.0, 2.0, Some(4.0));
    change.clock = None;
    change.start.beats = None;
    change.end = Some(NoteTime {
        frame: 96_000,
        beats: None,
    });
    history.record(1, played(vec![change]));
    let page = history.page("drone", None, None, RATE);
    assert_eq!((page.clock, page.playhead), (None, None));
    let drone = &page.notes[0].note;
    assert_eq!((drone.start_beats, drone.end_beats), (None, None));
    assert_eq!((drone.start_seconds, drone.end_seconds), (1.0, Some(2.0)));
}

#[test]
fn a_missed_change_forgets_the_notes_held_and_keeps_the_rest() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(
        1,
        played(vec![
            note("seq", 0, 0, 60.0, 0.0, Some(0.5)),
            note("seq", 1, 1, 64.0, 1.0, None),
        ]),
    );
    // Note 1's end may be among the missed: it would otherwise hold forever.
    history.record(
        1,
        NoteChanges {
            notes: Vec::new(),
            missed: 1,
        },
    );
    assert_eq!(ids(&poll(&history, "seq", None)), [("0".into(), Some(0.5))]);
}

#[test]
fn a_module_moving_to_another_clock_starts_its_history_over() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(1, played(vec![note("seq", 0, 0, 60.0, 4.0, None)]));
    let cursor = poll(&history, "seq", None).latest_seq;

    // Note 0 ends on the old clock; note 1 starts on the new one, whose
    // beats are another timeline.
    let mut moved = note("seq", 1, 0, 62.0, 0.5, None);
    moved.clock = Some("slow".to_string());
    history.record(
        1,
        played(vec![note("seq", 0, 0, 60.0, 4.0, Some(4.2)), moved]),
    );
    let page = history.page("seq", Some(cursor), on("slow", 0.6), RATE);
    assert!(
        page.resync_required,
        "what the reader holds is on the old clock"
    );
    assert_eq!(page.clock.as_deref(), Some("slow"));
    assert_eq!(ids(&page), [("1".into(), None)]);
    assert_eq!(page.playhead, Some(0.6));
    // A reader from here on is not resynced again.
    let next = history.page("seq", Some(page.latest_seq), on("slow", 0.7), RATE);
    assert!(!next.resync_required && next.notes.is_empty());
}

#[test]
fn a_playhead_on_another_clock_than_the_notes_is_left_out() {
    let mut history = NoteHistory::starting_at(1_000);
    history.record(1, played(vec![note("seq", 0, 0, 60.0, 4.0, Some(4.5))]));
    // The module has moved to `slow` but played nothing on it yet.
    let page = history.page("seq", None, on("slow", 0.2), RATE);
    assert_eq!(
        (page.clock.as_deref(), page.playhead),
        (Some("clock"), None)
    );
    assert_eq!(page.notes[0].note.start_beats, Some(4.0));
    // A module that has played nothing is on its playhead's clock.
    let idle = history.page("other", None, on("slow", 0.2), RATE);
    assert_eq!(
        (idle.clock.as_deref(), idle.playhead),
        (Some("slow"), Some(0.2))
    );
}
