//! The `velocity` output: the latest attack's velocity, moving only on the
//! sample the gate rises.

use super::*;
use crate::alloc_counter::allocator_events;

fn with_velocity(offset: i8, velocity: f32) -> Step {
    Step {
        velocity: Some(velocity),
        ..Step::note(offset)
    }
}

/// Clocks `seq` one sample at a time, an edge every `period` samples held
/// high for `high`, returning (gate, velocity) per sample.
fn play(seq: &mut StepSequencer, samples: usize, period: usize, high: usize) -> Vec<(f32, f32)> {
    (0..samples)
        .map(|n| {
            let clock = if n % period < high { 1.0 } else { 0.0 };
            seq.set_input("clock", clock).unwrap();
            seq.process(1);
            (
                seq.get_output("gate").unwrap(),
                seq.get_output("velocity").unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_retriggered_note_takes_its_velocity_as_the_gate_rises_not_on_the_dip() {
    // The first step's gate is over-estimated (no tempo measured yet), so
    // step 1 starts with a one-sample forced release.
    let mut seq = StepSequencer::new(1000)
        .with_step_count(2)
        .with_gate_length(0.95)
        .with_pattern(vec![with_velocity(0, 0.4), with_velocity(5, 0.9)]);
    let heard = play(&mut seq, 200, 100, 50);
    assert_eq!(heard[0], (1.0, 0.4));
    assert_eq!(heard[100], (0.0, 0.4), "the dip keeps the old velocity");
    assert_eq!(
        heard[101],
        (1.0, 0.9),
        "the new one arrives as the gate rises"
    );
}

#[test]
fn reset_and_a_finished_one_shot_keep_the_last_velocity() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(2)
        .with_one_shot(true)
        .with_pattern(vec![with_velocity(0, 0.6), with_velocity(2, 0.2)]);
    let heard = play(&mut seq, 300, 100, 50);
    assert_eq!(heard[150].1, 0.2);
    assert_eq!(heard[299], (0.0, 0.2), "ended, the last attack's");
    seq.set_input("reset", 1.0).unwrap();
    seq.process(1);
    assert_eq!(
        seq.get_output("velocity").unwrap(),
        0.2,
        "until a note strikes"
    );
}

#[test]
fn a_silent_step_strikes_at_zero() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(1)
        .with_pattern(vec![with_velocity(0, 0.0)]);
    assert_eq!(play(&mut seq, 1, 100, 50)[0], (1.0, 0.0));
}

#[test]
fn the_velocity_block_matches_the_port() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(1)
        .with_pattern(vec![with_velocity(0, 0.7)]);
    seq.set_input("clock", 1.0).unwrap();
    seq.process(4);
    let index = seq.outputs().iter().position(|&port| port == "velocity");
    assert_eq!(&seq.output_block(index.unwrap())[..4], &[0.7; 4]);
}

#[test]
fn playing_velocities_and_grace_notes_neither_allocates_nor_frees() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(3)
        .with_pattern(vec![
            with_velocity(0, 0.5),
            Step {
                velocity: Some(0.9),
                ..Step::note_with_grace(7, &[2, 4])
            },
            Step::note_with_grace(5, &[3]),
        ]);
    for on_beat in [0, 1] {
        seq.set(GRACE_PLACEMENT, RtValue::U32(on_beat));
        let ((), allocs, frees) = allocator_events(|| {
            for n in 0..1200 {
                let clock = if n % 100 < 50 { 1.0 } else { 0.0 };
                seq.set_input("clock", clock).unwrap();
                seq.process(1);
            }
        });
        assert_eq!((allocs, frees), (0, 0));
    }
}
