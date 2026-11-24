//! The first step after a tempo change, from the spec alone.
//!
//! A `step_sequencer` or `cell_sequencer` whose `clock` input is fed directly
//! by a clock module's gate output times the first step after a tempo change
//! alone (a write to the clock's `bpm` control, nothing else) like every
//! later step at the new tempo: its gate stays high for gate length times the
//! new step length, within 2 samples (sequencers measure the step as the
//! integer gap between clock edges). Steps before the change and steps after
//! the first one are unchanged.
//!
//! Each piece routes the clock's gate to the dac's left input and the
//! sequencer's gate to its right input and reads the rendered samples
//! (threshold 0.5). The `bpm` write is delivered offline (between two
//! `render_interleaved` calls) or live as a timed request (`AtSample`, or
//! the clock's next bar line).
//!
//! Run with `cargo test --test tempo_change_first_gate_acceptance`.

mod rig;

use rig::{Case, Delivery, Landing, Module, Step, TOLERANCE};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE as f64
}

fn gate_near(case: &Case, a: f64, b: f64) -> bool {
    (a - b).abs() <= case.slack() as f64
}

/// Checks one case and returns what is wrong with it, if anything.
fn check(case: &Case, module: Module, delivery: Delivery) -> Vec<String> {
    let rec = rig::record(case, module, delivery);
    let steps = rig::steps(&rec);
    let old_len = RATE_F * 60.0 / case.from_bpm;
    let new_len = RATE_F * 60.0 / case.to_bpm;
    let change = case.change_step();
    let mut problems = Vec::new();

    // The test is only meaningful if the clock really changed tempo where
    // the write was timed: the steps up to the change are at the old tempo,
    // and the first step to start after it is at the new one.
    let first_new = match case.landing {
        Landing::OnEdge { .. } => change,
        Landing::MidStep { .. } => change + 1,
    };
    for (k, step) in steps.iter().enumerate().take(change) {
        if !near(step.length() as f64, old_len) {
            problems.push(format!(
                "setup: step {k} is {} samples, old step is {old_len:.0}",
                step.length()
            ));
        }
    }
    match steps.get(first_new) {
        Some(step) if near(step.length() as f64, new_len) => {}
        other => problems.push(format!(
            "setup: step {first_new} should be one new step ({new_len:.0} samples) long, got {:?}",
            other.map(Step::length)
        )),
    }
    if steps.len() < first_new + AFTER {
        problems.push(format!("setup: only {} steps recorded", steps.len()));
        return problems;
    }
    if !problems.is_empty() {
        return problems;
    }

    // `unit` is the step length at the tempo the step's gate was sized at.
    // Spec point 4: steps before the change are unchanged (the one a mid-step
    // write lands in has its gate sized at its start, from the old tempo, and
    // the gate had ended before the write).
    for k in 0..first_new {
        let in_change = matches!(case.landing, Landing::MidStep { .. }) && k == change;
        problems.extend(gate_problem(case, &steps[k], k, old_len, in_change));
    }
    // The first step after the change, and every one after it, at the new
    // tempo.
    for k in first_new..first_new + AFTER {
        problems.extend(gate_problem(case, &steps[k], k, new_len, false));
    }
    problems
}

/// What is wrong with a step's gate: it should be gate length times the step
/// length: its own edge-to-edge length, or, for the step the change lands
/// in (`mixed`), whose edge gap mixes both tempos, the step length at the
/// tempo it was sized at (`unit`).
fn gate_problem(case: &Case, step: &Step, k: usize, unit: f64, mixed: bool) -> Option<String> {
    let high = match &step.high {
        Ok(high) => *high as f64,
        Err(e) => return Some(format!("step {k}: {e}")),
    };
    let want = case.gate_length * unit;
    let own = case.gate_length * step.length() as f64;
    if gate_near(case, high, if mixed { want } else { own }) {
        return None;
    }
    Some(format!(
        "step {k}: gate high {high} samples, expected about {want:.0} (gate length {} x a step \
         of {unit:.0} samples; this step's own edge gap is {} samples)",
        case.gate_length,
        step.length()
    ))
}

const RATE_F: f64 = rig::RATE as f64;

fn run(module: Module, delivery: Delivery, cases: &[Case]) {
    let mut failures = Vec::new();
    for case in cases {
        for p in check(case, module, delivery) {
            failures.push(format!("{case:?}:\n    {p}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{module:?} via {delivery:?}:\n{}",
        failures.join("\n")
    );
}

const AFTER: usize = rig::AFTER;

fn on_edge(from_bpm: f64, to_bpm: f64, gate_length: f64, step: usize) -> Case {
    Case {
        from_bpm,
        to_bpm,
        gate_length,
        landing: Landing::OnEdge { step },
    }
}

fn mid_step(from_bpm: f64, to_bpm: f64, gate_length: f64, step: usize) -> Case {
    Case {
        from_bpm,
        to_bpm,
        gate_length,
        landing: Landing::MidStep { step },
    }
}

/// Spec point 1: a write on a bar line (a clock edge), 120 to 60 bpm. On
/// main the first gate is about half as long as later ones.
fn slowing_on_edge(step: usize) -> Vec<Case> {
    vec![
        on_edge(120.0, 60.0, 0.5, step),
        on_edge(120.0, 60.0, 0.8, step),
    ]
}

/// Spec point 1: a write on a bar line, 60 to 120 bpm. On main the first
/// gate is sized from the old, doubled step: too long, into the next edge.
fn speeding_on_edge(step: usize) -> Vec<Case> {
    vec![
        on_edge(60.0, 120.0, 0.5, step),
        on_edge(60.0, 120.0, 0.8, step),
    ]
}

/// A step length that is not a whole number of samples (28800 to 41142.86).
fn fractional_on_edge(step: usize) -> Vec<Case> {
    vec![
        on_edge(100.0, 70.0, 0.5, step),
        on_edge(100.0, 70.0, 0.8, step),
    ]
}

/// Spec point 2: a write landing mid-step.
fn mid_step_cases() -> Vec<Case> {
    vec![
        mid_step(120.0, 60.0, 0.5, 5),
        mid_step(120.0, 60.0, 0.8, 5),
        mid_step(60.0, 120.0, 0.5, 5),
        mid_step(60.0, 120.0, 0.8, 5),
    ]
}

macro_rules! tests {
    ($($name:ident: $module:expr, $delivery:expr, $cases:expr;)*) => {$(
        #[test]
        fn $name() {
            run($module, $delivery, &$cases);
        }
    )*};
}

// Bar lines are every 4 beats, so the timed-to-the-bar cases use step 8.
tests! {
    step_slowing_on_edge_offline: Module::Step, Delivery::Offline, slowing_on_edge(6);
    cell_slowing_on_edge_offline: Module::Cell, Delivery::Offline, slowing_on_edge(6);
    step_speeding_on_edge_offline: Module::Step, Delivery::Offline, speeding_on_edge(6);
    cell_speeding_on_edge_offline: Module::Cell, Delivery::Offline, speeding_on_edge(6);
    step_fractional_on_edge_offline: Module::Step, Delivery::Offline, fractional_on_edge(6);
    cell_fractional_on_edge_offline: Module::Cell, Delivery::Offline, fractional_on_edge(6);

    step_slowing_on_edge_at_sample: Module::Step, Delivery::LiveAtSample, slowing_on_edge(6);
    cell_slowing_on_edge_at_sample: Module::Cell, Delivery::LiveAtSample, slowing_on_edge(6);
    step_speeding_on_edge_at_sample: Module::Step, Delivery::LiveAtSample, speeding_on_edge(6);
    cell_speeding_on_edge_at_sample: Module::Cell, Delivery::LiveAtSample, speeding_on_edge(6);

    step_slowing_on_bar_line: Module::Step, Delivery::LiveNextBar, slowing_on_edge(8);
    cell_slowing_on_bar_line: Module::Cell, Delivery::LiveNextBar, slowing_on_edge(8);
    step_speeding_on_bar_line: Module::Step, Delivery::LiveNextBar, speeding_on_edge(8);
    cell_speeding_on_bar_line: Module::Cell, Delivery::LiveNextBar, speeding_on_edge(8);

    step_mid_step_offline: Module::Step, Delivery::Offline, mid_step_cases();
    cell_mid_step_offline: Module::Cell, Delivery::Offline, mid_step_cases();
    step_mid_step_at_sample: Module::Step, Delivery::LiveAtSample, mid_step_cases();
    cell_mid_step_at_sample: Module::Cell, Delivery::LiveAtSample, mid_step_cases();
}
