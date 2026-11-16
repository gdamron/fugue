//! Spec points 4 and 5: a `cells.N` write to a playing sequencer is a control
//! write heard from the next step, with nothing rebuilt or restarted and
//! nothing else changed, sample for sample; how cells are chosen and repeated
//! is unchanged.
//!
//! A test renders one piece three ways from the same start: the original
//! (R), a piece holding the edited cell from the start (E), and the original
//! edited at a fixed block (A). An edit that lands before the changed step
//! plays leaves A equal to E, frame for frame.

use fugue::ControlValue;
use serde_json::{json, Value};

use super::{differences, looping, notes, three_cells, voice_piece, with_cell, Piece};

/// Frames in one step: sixteenth notes at 240 bpm.
const STEP: usize = 3000;
type Channels = (Vec<f32>, Vec<f32>);

/// Cell 0 with its fourth step (index 3) raised a semitone.
fn raised_fourth_step() -> Value {
    notes(&[0, 2, 4, 6, 7, 9, 11, 12])
}

fn extend(into: &mut Channels, (left, right): Channels) {
    into.0.extend(left);
    into.1.extend(right);
}

/// R, E and A for a looping cell 0 tapped at `right`, A edited by a batch
/// half a step before the changed step; also the step's onset on the left.
fn edit_while_playing(right: &str) -> (usize, [Channels; 3]) {
    let blocks = 360; // under one eight-step cycle (375 blocks)
    let start = |bank| Piece::from_json(&voice_piece(looping(bank), right));
    let original = start(three_cells()).render(blocks);
    let always = start(with_cell(three_cells(), 0, &raised_fourth_step())).render(blocks);
    let (onset, _) = differences(&original.0, &always.0).expect("the edited cell sounds different");
    assert!(onset > 2 * STEP, "the changed step is the fourth: {onset}");

    let at = (onset - STEP / 2) / 64;
    let mut edited = start(three_cells());
    let mut heard = edited.render(at);
    let report = edited
        .edit("seq", "cells.0", &raised_fourth_step())
        .expect("the batch applies");
    assert!(
        (report.controls_written.iter())
            .any(|c| c.module_id == "seq" && c.key.starts_with("cells")),
        "a control write: {report:?}"
    );
    assert!(report.rebuilt.is_empty(), "nothing rebuilt: {report:?}");
    assert!(
        report.added.is_empty() && report.removed.is_empty(),
        "{report:?}"
    );
    extend(&mut heard, edited.render(blocks - at));
    (onset, [original, always, heard])
}

#[test]
fn an_edit_to_the_playing_cell_is_heard_from_the_next_step_and_nothing_restarts() {
    let (onset, [original, always, edited]) = edit_while_playing("step");
    // The sequencer kept its position: its step output never moved.
    assert_eq!(
        differences(&edited.1, &original.1),
        None,
        "step keeps counting"
    );
    // Identical before the changed note, and the edit is heard.
    let (first, _) = differences(&edited.0, &original.0).expect("the edit is heard");
    assert!(
        first >= onset,
        "nothing changes before the changed step: {first} < {onset}"
    );
    // Exactly as a piece that always held the edited cell: no click, no
    // restart, no discontinuity anywhere.
    assert_eq!(
        differences(&edited.0, &always.0),
        None,
        "sounds as if always so"
    );
}

#[test]
fn only_the_changed_note_differs_in_what_the_sequencer_plays() {
    let (_, [original, always, edited]) = edit_while_playing("frequency");
    let (changed, _) = differences(&original.1, &always.1).expect("the cells differ");
    let (first, last) = differences(&edited.1, &original.1).expect("the pitch changes");
    assert!(
        first == changed && last < changed + STEP + STEP / 2,
        "only the changed step's pitch differs: {first}..{last}, step at {changed}"
    );
    assert_eq!(differences(&edited.1, &always.1), None);
}

#[test]
fn cells_that_did_not_change_play_exactly_as_before() {
    let start = || Piece::from_json(&voice_piece(looping(three_cells()), "step"));
    let original = start().render(800);
    let edited = start();
    let mut heard = edited.render(100);
    // Cell 1 is not playing; cell 0 loops on.
    edited
        .write("seq", "cells.1", &notes(&[5; 8]))
        .expect("cells.1 applies");
    extend(&mut heard, edited.render(700));
    assert_eq!(differences(&heard.0, &original.0), None, "heard as before");
    assert_eq!(
        differences(&heard.1, &original.1),
        None,
        "positioned as before"
    );
    assert_eq!(edited.read("seq", "cell"), json!(0.0), "cell 0 still plays");
}

#[test]
fn one_shot_playback_and_selection_are_unchanged_by_a_cell_edit() {
    let last_cell = notes(&[24, 19, 16, 12, 7, 4, 0, -12]);
    let one_shot = |bank| {
        let mut config = looping(bank);
        config["mode"] = json!("one_shot");
        Piece::from_json(&voice_piece(config, "cell"))
    };
    let edited = one_shot(three_cells());
    let always = one_shot(with_cell(three_cells(), 2, &last_cell));
    // Edit the last cell while the first plays; one-shot playback reaches it.
    let mut heard = edited.render(100);
    edited
        .write("seq", "cells.2", &last_cell)
        .expect("cells.2 applies");
    extend(&mut heard, edited.render(1200));
    let want = always.render(1300);
    assert_eq!(
        differences(&heard.0, &want.0),
        None,
        "the bank plays through as edited"
    );
    assert_eq!(
        differences(&heard.1, &want.1),
        None,
        "advancing cell by cell"
    );
    let mut cells: Vec<u32> = heard.1.iter().map(|x| x.to_bits()).collect();
    cells.dedup();
    assert!(
        cells.len() >= 3,
        "every cell played: {} values",
        cells.len()
    );

    // Selecting a cell re-arms playback, as before, in both.
    for piece in [&edited, &always] {
        let select = ControlValue::Number(1.0);
        piece
            .running
            .set_control("seq", "select_cell", select)
            .expect("select_cell applies");
    }
    let (heard, want) = (edited.render(300), always.render(300));
    assert_eq!(differences(&heard.0, &want.0), None);
    assert_eq!(edited.read("seq", "cell"), json!(1.0));
    assert!(
        heard.0.iter().any(|x| *x != 0.0),
        "the selected cell sounds"
    );
}
