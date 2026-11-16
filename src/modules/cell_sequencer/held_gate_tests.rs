//! The gate bridged into a held step releases when the clock stops, and
//! holds through a held edge up to two steps and two samples late.

use super::*;

/// Drives `seq` one sample per call with clock edges at `edges` and returns
/// the gate.
fn drive(seq: &mut CellSequencer, edges: &[usize], samples: usize) -> Vec<bool> {
    (0..samples)
        .map(|n| {
            let high = edges.iter().any(|&e| n >= e && n < e + 10);
            seq.set_input("clock", if high { 1.0 } else { 0.0 })
                .unwrap();
            seq.process(1);
            seq.get_output("gate").unwrap() > 0.5
        })
        .collect()
}

fn note_then_held() -> CellSequencer {
    CellSequencer::new(1000)
        .with_step_count(4)
        .with_cells(vec![vec![
            Step::rest(),
            Step::note(0),
            Step::held(),
            Step::rest(),
        ]])
}

/// The clock stops after the note's edge (step length 100): the gate holds
/// through 2 * 100 + 1 samples after that edge and is low from 2 * 100 + 2.
#[test]
fn a_stopped_clock_releases_the_bridge_at_two_steps_and_two_samples() {
    let mut seq = note_then_held();
    let gate = drive(&mut seq, &[0, 100], 1000);
    assert!(gate[100..=301].iter().all(|&g| g));
    assert!(gate[302..].iter().all(|&g| !g));
}

/// The tempo halves while the note sounds: the held edge comes 200, 201 or
/// 202 samples later and the gate stays high through it.
#[test]
fn a_bridge_survives_an_edge_two_steps_later() {
    for late in [200, 201, 202] {
        let mut seq = note_then_held();
        let gate = drive(&mut seq, &[0, 100, 100 + late], 100 + late + 50);
        assert!(
            gate[100..100 + late + 50].iter().all(|&g| g),
            "the gate stays high into a held edge {late} samples later"
        );
    }
}

/// A slowdown past half: the bridge has released, and the late held edge
/// re-attacks the note.
#[test]
fn a_held_edge_past_the_release_re_attacks_the_note() {
    let mut seq = note_then_held();
    let gate = drive(&mut seq, &[0, 100, 400], 450);
    assert!(gate[302..400].iter().all(|&g| !g));
    assert!(gate[400..450].iter().all(|&g| g));
}
