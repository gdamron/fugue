//! When the clock resumes after a release, the next edges behave as
//! documented, with no stuck or spurious gate.

use fugue::modules::Step;

use crate::rig::{assert_level, drive, rises, sequencer, D};

/// The clock stops after the note's edge (at D), the gate releases, and the
/// clock resumes at 7D, one edge per D from there:
///
/// - 7D, a held step continuing the note: the note re-attacks at that
///   edge, at its own pitch.
/// - 8D, a rest: low.
/// - 9D, a held step with no note sounding: silent.
/// - 10D, a note: a fresh rising edge, at its own pitch.
/// - 11D on, rests: low.
#[test]
fn a_resumed_clock_re_attacks_a_held_note_and_leaves_no_stuck_gate() {
    let cell = vec![
        Step::rest(),
        Step::note(0),
        Step::held(),
        Step::rest(),
        Step::held(),
        Step::note(7),
        Step::rest(),
        Step::rest(),
    ];
    let mut seq = sequencer(cell, false);
    let edges = [0, D, 7 * D, 8 * D, 9 * D, 10 * D, 11 * D, 12 * D, 13 * D];
    let trace = drive(&mut seq, &edges, 14 * D);
    let gate = &trace.gate;

    assert_level(gate, 3 * D + 2..7 * D, false, "released while stopped");
    assert_eq!(
        rises(&gate[..14 * D]),
        vec![D, 7 * D, 10 * D],
        "attacks: the note, its re-attack on the resumed held edge, the new note"
    );
    assert_level(gate, 7 * D..7 * D + D / 2, true, "the re-attacked note");
    assert_eq!(
        trace.frequency[7 * D + 1],
        trace.frequency[D + 1],
        "the re-attack is the held note's pitch"
    );
    assert_level(
        gate,
        8 * D + 1..10 * D,
        false,
        "the rest, then the held step after it",
    );
    assert!(
        trace.frequency[10 * D + 1] > trace.frequency[D + 1],
        "the new note sounds at its own, higher pitch"
    );
    assert_level(gate, 10 * D..10 * D + D / 4, true, "the new note");
    assert_level(gate, 11 * D + 1..14 * D, false, "the rests after it");
}
