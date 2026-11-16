//! The gate bridged into a held step: its release when no edge comes.

use super::*;

/// Drives `seq` at one sample per call with clock edges at `edges`, and
/// returns the gate.
fn drive(seq: &mut StepSequencer, edges: &[usize], samples: usize) -> Vec<bool> {
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

fn note_then_held() -> StepSequencer {
    StepSequencer::new(1000)
        .with_step_count(4)
        .with_pattern(vec![
            Step::rest(),
            Step::note(0),
            Step::held(),
            Step::rest(),
        ])
}

/// The tempo halves while the note sounds: the held step's edge comes two
/// measured steps (and one more sample) later, and the gate stays high.
#[test]
fn a_bridge_survives_an_edge_two_steps_later() {
    for late in [200, 201] {
        let mut seq = note_then_held();
        let gate = drive(&mut seq, &[0, 100, 100 + late], 100 + late + 50);
        assert!(
            gate[100..100 + late + 50].iter().all(|&g| g),
            "the gate stays high into a held edge {late} samples later"
        );
    }
}

/// With no further edge, the bridged gate releases rather than sticking.
#[test]
fn a_bridge_releases_when_the_clock_stops() {
    let mut seq = note_then_held();
    let gate = drive(&mut seq, &[0, 100], 600);
    assert!(gate[100..=300].iter().all(|&g| g));
    assert!(gate[301..].iter().all(|&g| !g));
}
