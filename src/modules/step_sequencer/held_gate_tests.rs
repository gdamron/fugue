//! The gate bridged into a held step holds through a late held edge. (Its
//! release on a stopped clock is in `tests/step_sequencer_held_gate`.)

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
/// measured steps (and up to two more samples) later, and the gate stays
/// high.
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

/// A clock at 97 BPM (sixteenths of 7,422.68 samples at 48 kHz) halves its
/// tempo just after the note's edge: the measured step rounds down and the
/// halved one up, so the held edge lands 2N + 2 samples later. The gate
/// holds through it.
#[test]
fn a_fractional_tempo_halving_keeps_the_held_gate() {
    use crate::modules::Clock;
    let mut clock = Clock::new(48_000, 97.0);
    let mut pattern = vec![Step::rest(); 16];
    pattern[13] = Step::note(0);
    pattern[14] = Step::held();
    let mut seq = StepSequencer::new(48_000)
        .with_step_count(16)
        .with_pattern(pattern);
    let (mut last_clock, mut edges, mut gate) = (0.0, Vec::new(), Vec::new());
    for n in 0..120_000 {
        clock.process(1);
        let beat = clock.get_output("beat_x4").unwrap();
        if beat > 0.5 && last_clock <= 0.5 {
            edges.push(n);
            if edges.len() == 14 {
                clock.set_control("bpm", 48.5).unwrap();
            }
        }
        last_clock = beat;
        seq.set_input("clock", beat).unwrap();
        seq.process(1);
        gate.push(seq.get_output("gate").unwrap() > 0.5);
    }
    let (note, held) = (edges[13], edges[14]);
    assert_eq!(held - note, 2 * (note - edges[12]) + 2, "edges {edges:?}");
    assert!(gate[note..held + 100].iter().all(|&g| g));
}
