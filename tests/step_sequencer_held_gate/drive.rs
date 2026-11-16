//! The `StepSequencer` module driven sample by sample through its public
//! `Module` API, for clocks that change tempo or stop, resets and pattern
//! writes landing at exact moments.

use fugue::modules::{Step, StepSequencer};
use fugue::Module;

/// One step is 100 samples at 1 kHz.
const RATE: u32 = 1_000;
const D: usize = 100;
/// The clock input stays high this long after each edge.
const PULSE: usize = 10;

/// What the sequencer put out on each sample.
struct Trace {
    gate: Vec<f32>,
    frequency: Vec<f32>,
    velocity: Vec<f32>,
}

/// Plays `seq` for `total` samples with clock edges at `edges`, calling
/// `event(sample, seq)` before each sample is processed (to write a
/// pattern, say), and raising `reset` at `reset_at`.
fn drive(
    seq: &mut StepSequencer,
    edges: &[usize],
    reset_at: Option<usize>,
    total: usize,
    mut event: impl FnMut(usize, &mut StepSequencer),
) -> Trace {
    let mut trace = Trace {
        gate: Vec::with_capacity(total),
        frequency: Vec::with_capacity(total),
        velocity: Vec::with_capacity(total),
    };
    for t in 0..total {
        event(t, seq);
        let clock = edges.iter().any(|&e| t >= e && t < e + PULSE);
        seq.set_input("clock", if clock { 1.0 } else { 0.0 })
            .unwrap();
        let reset = reset_at.is_some_and(|r| t >= r && t < r + PULSE);
        seq.set_input("reset", if reset { 1.0 } else { 0.0 })
            .unwrap();
        seq.process(1);
        trace.gate.push(seq.get_output("gate").unwrap());
        trace.frequency.push(seq.get_output("frequency").unwrap());
        trace.velocity.push(seq.get_output("velocity").unwrap());
    }
    trace
}

fn every(step: usize, count: usize) -> Vec<usize> {
    (0..count).map(|k| k * step).collect()
}

fn with_velocity(offset: i8, velocity: f32) -> Step {
    let mut step = Step::note(offset);
    step.velocity = Some(velocity);
    step
}

/// rest, note, held, held, then rests: an 8-step loop.
fn held_run_pattern() -> Vec<Step> {
    vec![
        Step::rest(),
        Step::note(0),
        Step::held(),
        Step::held(),
        Step::rest(),
        Step::rest(),
        Step::rest(),
        Step::rest(),
    ]
}

fn sequencer(pattern: Vec<Step>) -> StepSequencer {
    StepSequencer::new(RATE)
        .with_root_note(60)
        .with_step_count(pattern.len())
        .with_gate_length(0.5)
        .with_pattern(pattern)
}

fn assert_level(gate: &[f32], range: std::ops::Range<usize>, high: bool, what: &str) {
    if let Some(i) = range.clone().find(|&i| (gate[i] > 0.5) != high) {
        let level = if high { "high" } else { "low" };
        panic!("{what}: the gate should be {level} over {range:?} but is not at sample {i}");
    }
}

/// Frequency and velocity hold, unchanged, from the note's
/// attack through its held steps.
#[test]
fn frequency_and_velocity_hold_through_held_steps() {
    let mut seq = sequencer(vec![
        with_velocity(0, 0.3),
        with_velocity(4, 0.6),
        Step::held(),
        Step::held(),
        Step::rest(),
    ]);
    let edges = every(D, 12);
    let trace = drive(&mut seq, &edges, None, 11 * D, |_, _| {});
    for pass in 0..2 {
        let note = edges[5 * pass + 1];
        let end = edges[5 * pass + 4] - 1;
        let pitch = trace.frequency[note + 1];
        assert!(pitch > 0.0, "the note sounds");
        for i in note + 1..end {
            assert_eq!(trace.frequency[i], pitch, "pass {pass}: frequency at {i}");
            assert!(
                (trace.velocity[i] - 0.6).abs() < 1e-6,
                "pass {pass}: velocity {} at {i}",
                trace.velocity[i]
            );
        }
    }
}

/// Reset during the note (before its held step) and during
/// the hold drops the gate, and it stays low until the next clock edge.
#[test]
fn reset_releases_a_bridged_gate() {
    for reset in [D + D / 2, 2 * D + D / 2] {
        let mut seq = sequencer(held_run_pattern());
        let edges = every(D, 10);
        let trace = drive(&mut seq, &edges, Some(reset), 6 * D, |_, _| {});
        let next_edge = edges.iter().copied().find(|&e| e > reset).unwrap();
        assert_level(
            &trace.gate,
            reset..next_edge,
            false,
            &format!("after a reset at {reset}"),
        );
    }
}

/// The end of a one-shot pattern releases a held run.
#[test]
fn the_end_of_a_one_shot_releases_a_held_run() {
    let mut seq = sequencer(vec![
        Step::rest(),
        Step::note(0),
        Step::held(),
        Step::held(),
    ])
    .with_one_shot(true);
    let edges = every(D, 10);
    let trace = drive(&mut seq, &edges, None, 9 * D, |_, _| {});
    assert!(trace.gate[D + 1] > 0.5, "the note sounds");
    assert_level(&trace.gate, 4 * D..9 * D, false, "after the one-shot ends");
}

/// A pattern write that turns the upcoming held step into a
/// rest, mid-hold, leaves the gate low on that rest. Written during the
/// note (before its first held step) and during the first held step.
#[test]
fn writing_a_rest_over_the_upcoming_held_step_releases_the_gate() {
    for (write_at, replaced) in [(D + D / 2, 2), (2 * D + D / 2, 3)] {
        let mut seq = sequencer(held_run_pattern());
        let edges = every(D, 10);
        let trace = drive(&mut seq, &edges, None, 8 * D, |t, seq| {
            if t == write_at {
                let mut pattern = held_run_pattern();
                for step in pattern.iter_mut().skip(replaced) {
                    if step.held {
                        *step = Step::rest();
                    }
                }
                seq.set_pattern(pattern);
            }
        });
        assert_level(
            &trace.gate,
            edges[replaced]..8 * D,
            false,
            &format!("the rest written over step {replaced} at {write_at}"),
        );
    }
}

/// A pattern write that turns the upcoming held step into a
/// note, mid-hold, gives that note a fresh attack: a low sample, then high
/// at its edge, at its own pitch.
#[test]
fn writing_a_note_over_the_upcoming_held_step_re_attacks() {
    for (write_at, replaced) in [(D + D / 2, 2), (2 * D + D / 2, 3)] {
        let mut seq = sequencer(held_run_pattern());
        let edges = every(D, 10);
        let trace = drive(&mut seq, &edges, None, 6 * D, |t, seq| {
            if t == write_at {
                let mut pattern = held_run_pattern();
                pattern[replaced] = Step::note(7);
                seq.set_pattern(pattern);
            }
        });
        let edge = edges[replaced];
        let what = format!("the note written over step {replaced} at {write_at}");
        let dip = (edge - 2..=edge).find(|&i| trace.gate[i] <= 0.5);
        assert!(
            dip.is_some(),
            "{what}: no low sample just before its attack at {edge}: {:?}",
            &trace.gate[edge - 3..edge + 2]
        );
        assert!(trace.gate[edge + 1] > 0.5, "{what}: the new note sounds");
        assert!(
            trace.frequency[edge + 1] > trace.frequency[edge - 5],
            "{what}: at its own, higher pitch ({} after, {} before)",
            trace.frequency[edge + 1],
            trace.frequency[edge - 5]
        );
    }
}

/// A tempo change during the hold (clock edges speeding up
/// or slowing down) still ends the gate low on the rest after the run.
#[test]
fn a_tempo_change_during_the_hold_still_releases_after_the_run() {
    // Edges every D through the note, then every `after` from the first
    // held step on.
    for after in [D / 2, 2 * D] {
        let mut edges = vec![0, D, 2 * D];
        while edges.len() < 12 {
            edges.push(edges.last().unwrap() + after);
        }
        let mut seq = sequencer(held_run_pattern());
        let total = edges[8];
        let trace = drive(&mut seq, &edges, None, total, |_, _| {});
        assert!(trace.gate[2 * D - 5] > 0.5, "the note sounds");
        // Steps 4..8 are rests.
        assert_level(
            &trace.gate,
            edges[4]..edges[8],
            false,
            &format!("the rests after the hold, edges every {after} once held"),
        );
    }
}

/// When the clock stops (no more edges), a gate bridged
/// toward a held step holds two measured step durations past the last
/// edge, plus at most two samples, then releases. Stopped during the note,
/// and during the first held step.
#[test]
fn a_stopped_clock_releases_a_bridged_gate_within_two_steps() {
    for last in [D, 2 * D] {
        let edges: Vec<usize> = (0..=last / D).map(|k| k * D).collect();
        let mut seq = sequencer(held_run_pattern());
        let trace = drive(&mut seq, &edges, None, last + 8 * D, |_, _| {});
        // The bridge waits two measured steps (plus up to two samples for
        // fractional step lengths) for the held edge, then releases.
        assert_level(
            &trace.gate,
            last..last + 2 * D + 2,
            true,
            &format!("the bridge after the edge at {last}, awaiting its held step"),
        );
        assert_level(
            &trace.gate,
            last + 2 * D + 2..last + 8 * D,
            false,
            &format!("the clock stopped after the edge at {last}"),
        );
    }
}
