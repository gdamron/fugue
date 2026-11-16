//! A stopped clock releases a gate bridged toward a held step two measured
//! steps plus two samples after the last edge, and the gate stays low.

use fugue::modules::Step;

use crate::rig::{assert_level, assert_release, drive, sequencer, steady, D};

/// An 8-step cell of rests with `note` on step `at` and held steps after it
/// for `held` steps.
pub fn cell(at: usize, note: Step, held: usize) -> Vec<Step> {
    let mut cell = vec![Step::rest(); 8];
    cell[at] = note;
    for step in &mut cell[at + 1..=at + held] {
        *step = Step::held();
    }
    cell
}

/// The clock stops right after the note's edge: the gate holds through
/// last + 2D + 1 and is low from last + 2D + 2 for as long as the clock
/// stays stopped. D is the gap between the last two edges, checked for a
/// steady clock and for a last gap longer than the ones before it.
#[test]
fn a_stopped_clock_releases_the_note_bridged_into_a_held_step() {
    let cases = [(1, steady(D), D), (3, vec![0, D, 2 * D, 2 * D + 130], 130)];
    for (at, edges, d) in cases {
        let last = *edges.last().unwrap();
        let mut seq = sequencer(cell(at, Step::note(0), 1), false);
        let trace = drive(&mut seq, &edges, last + 20 * D);
        let what = format!("clock stopped after the note's edge at {last} (step of {d})");
        assert_level(&trace.gate, last..last + d, true, &what);
        assert_release(&trace.gate, last, d, last + 20 * D, &what);
    }
}

/// The clock stops during a held step that is itself followed by another
/// held step: the same release, timed from that held step's edge.
#[test]
fn a_stopped_clock_releases_a_held_step_bridged_into_another() {
    let last = 2 * D;
    let mut seq = sequencer(cell(1, Step::note(0), 2), false);
    let trace = drive(&mut seq, &steady(last), last + 20 * D);
    let what = "clock stopped during the first of two held steps";
    assert_level(&trace.gate, D..last, true, what);
    assert_release(&trace.gate, last, D, last + 20 * D, what);
}

/// An on-beat grace chain defers its principal, which bridges into a held
/// step; the clock stops after the decorated step's edge. The principal
/// sounds after the chain and releases on the same rule.
#[test]
fn a_stopped_clock_releases_a_principal_deferred_behind_on_beat_graces() {
    let last = D;
    let mut seq = sequencer(cell(1, Step::note_with_grace(0, &[2, 4]), 1), true);
    let trace = drive(&mut seq, &steady(last), last + 20 * D);
    let (grace, principal) = (trace.frequency[last + 1], trace.frequency[last + D + 5]);
    assert!(
        principal > 0.0 && grace > principal,
        "a grace sounds on the edge ({grace}), then the principal ({principal})"
    );
    assert_release(
        &trace.gate,
        last,
        D,
        last + 20 * D,
        "clock stopped after an on-beat grace chain's edge",
    );
}
