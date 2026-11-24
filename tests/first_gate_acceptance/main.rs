//! The first gate of a clock-driven sequencer, from the spec alone.
//!
//! A piece driven by a `step_sequencer` or a `cell_sequencer` whose `clock`
//! input is a clock module's gate output must give its first step's gate the
//! same length as the same step's gate on every later pass of the pattern
//! (within 2 samples), whether the step is longer or shorter than half a
//! second and whatever the gate length (the sequencer's or a step's own). No
//! other step's timing changes: every step's gate rises on its clock edge
//! and lasts the gate length times the step length (within 2 samples).
//!
//! Each piece routes the clock's gate to the dac's left input and the
//! sequencer's gate to its right input, and reads the rendered samples
//! (threshold 0.5), on the offline render and on the live runtime driven
//! block by block through a manual `AudioBackend`.
//!
//! Run with `cargo test --test first_gate_acceptance`.

mod rig;

use rig::{Case, Module, Surface, PASSES, STEPS, TOLERANCE};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE as f64
}

/// The spec's "first gate matches later passes", and that it is the right
/// length: for each case, the first step's gate against the same step's
/// gate on every later pass and against gate length times step length.
fn first_gate_matches_later_passes(module: Module, surface: Surface, cases: &[Case]) {
    let mut failures = Vec::new();
    for case in cases {
        let rec = rig::record(case, module, surface);
        let steps = match rig::steps(&rec) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{case:?}: {e}"));
                continue;
            }
        };
        let first = steps[0].high;
        let later: Vec<usize> = (1..PASSES).map(|p| steps[p * STEPS].high).collect();
        let expected = case.length_of(0) * steps[0].length() as f64;
        let report = format!(
            "{case:?}: first gate {first} samples, same step on later passes {later:?}, \
             expected about {expected:.0} (step is {} samples)",
            steps[0].length()
        );
        let matches_later = later.iter().all(|&l| near(first as f64, l as f64));
        if !matches_later || !near(first as f64, expected) {
            failures.push(report);
        }
    }
    assert!(
        failures.is_empty(),
        "{module:?} on {surface:?}:\n{}",
        failures.join("\n")
    );
}

/// "No other step's timing changes": every step's gate rises on its clock
/// edge and lasts gate length times step length, on the first pass and
/// later ones. The first step's first gate length is the other tests' job.
fn other_steps_keep_their_timing(module: Module, surface: Surface, cases: &[Case]) {
    let mut failures = Vec::new();
    for case in cases {
        let rec = rig::record(case, module, surface);
        let steps = match rig::steps(&rec) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{case:?}: {e}"));
                continue;
            }
        };
        for (k, step) in steps.iter().enumerate() {
            let late = step.rise as i64 - step.edge as i64;
            if late.abs() > TOLERANCE {
                failures.push(format!(
                    "{case:?} step {k}: gate rose {late} samples off its edge"
                ));
            }
            if k == 0 {
                continue;
            }
            let expected = case.length_of(k) * step.length() as f64;
            if !near(step.high as f64, expected) {
                failures.push(format!(
                    "{case:?} step {k}: gate {} samples, expected about {expected:.0}",
                    step.high
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{module:?} on {surface:?}:\n{}",
        failures.join("\n")
    );
}

/// Steps longer than half a second.
fn long_steps() -> Vec<Case> {
    vec![
        Case::plain(72.0, "beat", 0.5),
        Case::plain(72.0, "beat", 0.8),
        Case::plain(60.0, "beat", 0.5),
        Case::plain(60.0, "beat", 0.8),
    ]
}

/// Steps shorter than half a second.
fn short_steps() -> Vec<Case> {
    vec![
        Case::plain(150.0, "beat_x4", 0.5),
        Case::plain(150.0, "beat_x4", 0.8),
    ]
}

/// A gate length on each step, over the default, at both step lengths.
fn per_step_lengths() -> Vec<Case> {
    vec![
        Case::per_step(72.0, "beat", 0.5, [0.8, 0.3, 0.6, 0.4]),
        Case::per_step(72.0, "beat", 0.8, [0.3, 0.8, 0.5, 0.6]),
        Case::per_step(150.0, "beat_x4", 0.5, [0.8, 0.3, 0.6, 0.4]),
        Case::per_step(150.0, "beat_x4", 0.8, [0.3, 0.8, 0.5, 0.6]),
    ]
}

fn all_cases() -> Vec<Case> {
    let mut all = long_steps();
    all.extend(short_steps());
    all.extend(per_step_lengths());
    all
}

macro_rules! first_gate_tests {
    ($($name:ident: $module:expr, $surface:expr, $cases:expr;)*) => {$(
        #[test]
        fn $name() {
            first_gate_matches_later_passes($module, $surface, &$cases);
        }
    )*};
}

macro_rules! timing_tests {
    ($($name:ident: $module:expr, $surface:expr;)*) => {$(
        #[test]
        fn $name() {
            other_steps_keep_their_timing($module, $surface, &all_cases());
        }
    )*};
}

// The step sequencer, offline and live.
first_gate_tests! {
    step_offline_long_steps_first_gate_matches_later: Module::Step, Surface::Offline, long_steps();
    step_offline_short_steps_first_gate_matches_later: Module::Step, Surface::Offline, short_steps();
    step_offline_per_step_first_gate_matches_later: Module::Step, Surface::Offline, per_step_lengths();
    step_live_long_steps_first_gate_matches_later: Module::Step, Surface::Live, long_steps();
    step_live_short_steps_first_gate_matches_later: Module::Step, Surface::Live, short_steps();
    step_live_per_step_first_gate_matches_later: Module::Step, Surface::Live, per_step_lengths();
}

// The same for the cell sequencer.
first_gate_tests! {
    cell_offline_long_steps_first_gate_matches_later: Module::Cell, Surface::Offline, long_steps();
    cell_offline_short_steps_first_gate_matches_later: Module::Cell, Surface::Offline, short_steps();
    cell_offline_per_step_first_gate_matches_later: Module::Cell, Surface::Offline, per_step_lengths();
    cell_live_long_steps_first_gate_matches_later: Module::Cell, Surface::Live, long_steps();
    cell_live_short_steps_first_gate_matches_later: Module::Cell, Surface::Live, short_steps();
    cell_live_per_step_first_gate_matches_later: Module::Cell, Surface::Live, per_step_lengths();
}

// No other step's timing changes, for either sequencer.
timing_tests! {
    step_offline_other_steps_keep_their_timing: Module::Step, Surface::Offline;
    step_live_other_steps_keep_their_timing: Module::Step, Surface::Live;
    cell_offline_other_steps_keep_their_timing: Module::Cell, Surface::Offline;
    cell_live_other_steps_keep_their_timing: Module::Cell, Surface::Live;
}
