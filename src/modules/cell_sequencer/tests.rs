use super::*;
use crate::{ControlSurface, ControlValue, ModuleRegistry};

fn pulse(module: &mut CellSequencer, port: &str) {
    module.set_input(port, 1.0).unwrap();
    module.process(1);
    module.set_input(port, 0.0).unwrap();
    module.process(1);
}

fn advance_gate(module: &mut CellSequencer) {
    pulse(module, "gate");
}

#[test]
fn test_cell_sequencer_basic_playback() {
    let mut seq = CellSequencer::new(44_100).with_sequences(vec![
        vec![Step::note(0), Step::rest(), Step::note(7)],
        vec![Step::note(12)],
    ]);

    advance_gate(&mut seq);
    assert!(seq.get_output("frequency").unwrap() > 0.0);
    assert_eq!(seq.get_output("sequence").unwrap(), 0.0);

    advance_gate(&mut seq);
    assert_eq!(seq.get_output("step").unwrap(), 1.0);
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
}

#[test]
fn test_cell_sequencer_held_steps_continue_active_note() {
    let mut seq = CellSequencer::new(10)
        .with_steps(3)
        .with_gate_length(0.4)
        .with_sequences(vec![vec![Step::note(0), Step::held(), Step::rest()]]);

    advance_gate(&mut seq);
    let expected = Note::new(DEFAULT_BASE_NOTE).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    for _ in 0..2 {
        seq.process(1);
    }
    assert_eq!(
        seq.get_output("gate").unwrap(),
        1.0,
        "note followed by a held step should use a full-step gate"
    );

    advance_gate(&mut seq);
    assert_eq!(seq.current_step(), 1);
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    advance_gate(&mut seq);
    assert_eq!(seq.current_step(), 2);
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_cell_sequencer_held_chain_keeps_gate_high_across_step_boundaries() {
    // Regression: a held chain used to produce a one-sample gate dip at
    // every step boundary, which downstream ADSRs saw as a rising edge
    // and retriggered. With the fix the gate must stay continuously high
    // through the middle of any held chain.
    let mut seq = CellSequencer::new(48_000)
        .with_steps(5)
        .with_sequences(vec![vec![
            Step::note(0),
            Step::held(),
            Step::held(),
            Step::held(),
            Step::held(),
        ]]);

    const HIGH: usize = 3;
    const LOW: usize = 5;
    const PERIOD: usize = HIGH + LOW;

    // Drive two complete clock periods so step_duration_samples gets
    // calibrated from samples_since_gate.
    for _ in 0..2 {
        seq.set_input("gate", 1.0).unwrap();
        for _ in 0..HIGH {
            seq.process(1);
        }
        seq.set_input("gate", 0.0).unwrap();
        for _ in 0..LOW {
            seq.process(1);
        }
    }
    assert_eq!(seq.step_duration_samples as usize, PERIOD);

    // Now walk through two more periods (each one lands the sequencer in
    // a *middle* held step — the cell still has more held steps after).
    // The output gate must remain 1.0 every sample.
    for cycle in 0..2 {
        seq.set_input("gate", 1.0).unwrap();
        for sample in 0..HIGH {
            seq.process(1);
            assert_eq!(
                seq.get_output("gate").unwrap(),
                1.0,
                "held chain dropped gate during gate-high phase \
                 (cycle {}, sample {})",
                cycle,
                sample
            );
        }
        seq.set_input("gate", 0.0).unwrap();
        for sample in 0..LOW {
            seq.process(1);
            assert_eq!(
                seq.get_output("gate").unwrap(),
                1.0,
                "held chain dropped gate during gate-low phase \
                 (cycle {}, sample {})",
                cycle,
                HIGH + sample
            );
        }
    }
}

#[test]
fn test_held_step_before_new_note_releases_so_it_retriggers() {
    // Regression (FUG-189): a held step followed by a *new note* must release
    // its gate before the next clock edge so the note retriggers. It used to
    // fill the full step duration and rely on a one-sample boundary dip, which
    // vanishes at fractional step periods (fast tempos) — dropping the
    // retrigger and flattening the articulation. A held step followed by a
    // rest or the end of the chain still sustains fully
    // (see the held-chain and held-then-rest tests).
    let mut seq = CellSequencer::new(48_000)
        .with_steps(4)
        .with_gate_length(0.5)
        .with_sequences(vec![vec![
            Step::note(0),
            Step::held(),
            Step::note(7),
            Step::rest(),
        ]]);

    const HIGH: usize = 4;
    const LOW: usize = 6;
    const PERIOD: usize = HIGH + LOW;
    let edge = |seq: &mut CellSequencer| {
        seq.set_input("gate", 1.0).unwrap();
        seq.process(HIGH);
        seq.set_input("gate", 0.0).unwrap();
        seq.process(LOW);
    };

    edge(&mut seq); // step 0: note(0); next is held -> gate bridges (continuous)
    edge(&mut seq); // step 1: held; next is note(7) -> must release before the edge
    assert_eq!(seq.step_duration_samples as usize, PERIOD);
    assert_eq!(
        seq.get_output("gate").unwrap(),
        0.0,
        "held step before a new note must release so the note can retrigger"
    );

    // step 2: note(7) must retrigger — the gate rises again on its edge.
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 2);
    assert!(
        seq.get_output("gate").unwrap() > 0.5,
        "the new note's gate must rise (retrigger) after the held step released"
    );
}

#[test]
fn test_cell_sequencer_contextless_held_step_is_rest() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(1)
        .with_sequences(vec![vec![Step::held()]]);

    advance_gate(&mut seq);

    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_cell_sequencer_sequence_change_clears_held_state() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![
            vec![Step::note(0), Step::held()],
            vec![Step::held(), Step::rest()],
        ]);

    advance_gate(&mut seq);
    pulse(&mut seq, "next_sequence");

    assert_eq!(seq.current_sequence(), 1);
    assert_eq!(seq.current_step(), 0);
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_cell_sequencer_next_sequence_switches_immediately() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(3)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2), Step::note(4)],
            vec![Step::note(12), Step::note(14), Step::note(16)],
        ]);

    advance_gate(&mut seq);
    pulse(&mut seq, "next_sequence");

    assert_eq!(seq.current_sequence(), 1);
    assert_eq!(seq.current_step(), 0);
    assert_eq!(seq.get_output("sequence").unwrap(), 1.0);
    let expected = Note::new(60).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);

    advance_gate(&mut seq);
    let expected = Note::new(62).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
}

#[test]
fn test_cell_sequencer_waits_for_cycle_end_before_switching() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(3)
        .with_wait_for_cycle_end(true)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2), Step::note(4)],
            vec![Step::note(12), Step::note(14), Step::note(16)],
        ]);

    advance_gate(&mut seq);
    advance_gate(&mut seq);
    pulse(&mut seq, "next_sequence");

    assert_eq!(seq.current_sequence(), 0);
    assert_eq!(seq.current_step(), 1);

    advance_gate(&mut seq);
    assert_eq!(seq.current_sequence(), 0);
    assert_eq!(seq.current_step(), 2);

    advance_gate(&mut seq);
    assert_eq!(seq.current_sequence(), 1);
    assert_eq!(seq.current_step(), 0);
    let expected = Note::new(60).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
}

#[test]
fn test_cell_sequencer_wait_for_cycle_end_input_overrides_control() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(12)],
        ]);

    advance_gate(&mut seq);
    seq.set_input("wait_for_cycle_end", 1.0).unwrap();
    seq.set_input("next_sequence", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.current_sequence(), 0);
    assert_eq!(seq.pending_sequence, Some(1));

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    seq.set_input("gate", 0.0).unwrap();
    seq.process(1);

    assert_eq!(seq.current_sequence(), 0);

    advance_gate(&mut seq);
    assert_eq!(seq.current_sequence(), 1);
}

#[test]
fn test_cell_sequencer_selected_sequence_control_queues_latest_request() {
    let controls = CellSequencerControls::new_with_values(
        DEFAULT_BASE_NOTE,
        2,
        DEFAULT_GATE_LENGTH,
        0,
        true,
        vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(4), Step::note(5)],
            vec![Step::note(7), Step::note(9)],
        ],
    );
    let mut seq = CellSequencer::new_with_controls(44_100, controls.clone());

    advance_gate(&mut seq);
    controls
        .set_control("selected_sequence", ControlValue::Number(1.0))
        .unwrap();
    seq.process(1);
    controls
        .set_control("selected_sequence", ControlValue::Number(2.0))
        .unwrap();
    seq.process(1);

    assert_eq!(seq.pending_sequence, Some(2));

    advance_gate(&mut seq);
    advance_gate(&mut seq);
    assert_eq!(seq.current_sequence(), 2);
}

#[test]
fn test_sequences_json_round_trip() {
    let controls = CellSequencerControls::new();
    controls
        .set_control(
            "sequences_json",
            ControlValue::String(
                r#"[[{"note":0},{"note":null}],[{"note":12,"gate":0.5}]]"#.to_string(),
            ),
        )
        .unwrap();

    let ControlValue::String(value) = controls.get_control("sequences_json").unwrap() else {
        panic!("sequences_json should be a string");
    };
    let parsed: Value = serde_json::from_str(&value).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
}

#[test]
fn test_advance_control_advances_cell_and_resets_loop_count() {
    let controls = CellSequencerControls::new_with_values(
        DEFAULT_BASE_NOTE,
        2,
        DEFAULT_GATE_LENGTH,
        0,
        false,
        vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(7), Step::note(9)],
        ],
    );
    let mut seq = CellSequencer::new_with_controls(44_100, controls.clone());

    advance_gate(&mut seq);
    advance_gate(&mut seq);
    advance_gate(&mut seq); // wraps cell 0 once
    assert_eq!(controls.loop_count(), 1);
    assert_eq!(controls.current_cell(), 0);

    controls
        .set_control("advance", ControlValue::Number(1.0))
        .unwrap();
    seq.process(1);

    assert_eq!(seq.current_sequence(), 1);
    assert_eq!(controls.current_cell(), 1);
    assert_eq!(controls.loop_count(), 0);
    assert_eq!(controls.total_cells(), 2);
}

#[test]
fn test_loop_count_increments_on_cell_wrap() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![vec![Step::note(0), Step::note(2)]]);

    advance_gate(&mut seq);
    advance_gate(&mut seq);
    assert_eq!(seq.ctrl.loop_count(), 0);
    advance_gate(&mut seq); // wraps to step 0 → loop completed
    assert_eq!(seq.ctrl.loop_count(), 1);
    advance_gate(&mut seq);
    advance_gate(&mut seq); // wraps again
    assert_eq!(seq.ctrl.loop_count(), 2);
}

#[test]
fn test_cell_sequencer_factory_and_registry() {
    let factory = CellSequencerFactory;
    let result = factory
        .build(
            44_100,
            &serde_json::json!({
                "steps": 4,
                "selected_sequence": 1,
                "wait_for_cycle_end": true,
                "sequences": [
                    [{ "note": 0 }],
                    [{ "note": 12 }]
                ]
            }),
        )
        .unwrap();

    assert!(result.control_surface.is_some());
    assert_eq!(
        result.module.module().outputs(),
        &["frequency", "gate", "velocity", "step", "sequence", "end"]
    );

    let registry = ModuleRegistry::default();
    assert!(registry.has_type("cell_sequencer"));
}

#[test]
fn test_one_shot_plays_bank_through_and_fires_end() {
    // Three cells x 2 steps: one_shot concatenates them into one sequence.
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(4), Step::note(5)],
            vec![Step::note(7), Step::note(9)],
        ])
        .with_one_shot(true);

    let expected_cells = [0.0, 0.0, 1.0, 1.0, 2.0, 2.0];
    for (i, expected_cell) in expected_cells.iter().enumerate() {
        advance_gate(&mut seq);
        assert_eq!(
            seq.get_output("sequence").unwrap(),
            *expected_cell,
            "cell at clock {}",
            i
        );
        assert_eq!(
            seq.get_output("end").unwrap(),
            0.0,
            "end low at clock {}",
            i
        );
        assert!(seq.get_output("frequency").unwrap() > 0.0);
    }

    // Next clock edge completes the final step of the final cell.
    advance_gate(&mut seq);
    assert_eq!(
        seq.get_output("end").unwrap(),
        1.0,
        "end fires at bank completion"
    );
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);

    // Latched; further clocks ignored.
    for _ in 0..3 {
        advance_gate(&mut seq);
        assert_eq!(seq.get_output("end").unwrap(), 1.0);
        assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    }
}

#[test]
fn test_one_shot_reset_rearms_current_cell() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(4), Step::note(5)],
        ])
        .with_one_shot(true);

    for _ in 0..5 {
        advance_gate(&mut seq);
    }
    assert_eq!(seq.get_output("end").unwrap(), 1.0);

    // Reset clears the latch; playback resumes in the final cell.
    pulse(&mut seq, "reset");
    assert_eq!(seq.get_output("end").unwrap(), 0.0);
    advance_gate(&mut seq);
    assert!(seq.get_output("frequency").unwrap() > 0.0);
    assert_eq!(seq.get_output("sequence").unwrap(), 1.0);
}

#[test]
fn test_one_shot_explicit_selection_rearms_and_restarts() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(4), Step::note(5)],
        ])
        .with_one_shot(true);

    for _ in 0..5 {
        advance_gate(&mut seq);
    }
    assert_eq!(seq.get_output("end").unwrap(), 1.0);

    // Selecting cell 0 re-arms even with wait_for_cycle_end semantics (no
    // cycle is running while finished).
    seq.set_control("selected_sequence", 0.0).unwrap();
    seq.process(1);
    assert_eq!(seq.get_output("end").unwrap(), 0.0, "selection re-arms");

    // Selection primes step 0 immediately (as with live cell switches), so
    // the first clock advances to step 1; the bank then plays through and
    // ends again.
    for expected_cell in [0.0, 1.0, 1.0] {
        advance_gate(&mut seq);
        assert_eq!(seq.get_output("sequence").unwrap(), expected_cell);
    }
    advance_gate(&mut seq);
    assert_eq!(
        seq.get_output("end").unwrap(),
        1.0,
        "second playthrough ends"
    );
}

#[test]
fn test_one_shot_pending_command_overrides_auto_advance() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_wait_for_cycle_end(true)
        .with_sequences(vec![
            vec![Step::note(0), Step::note(2)],
            vec![Step::note(4), Step::note(5)],
            vec![Step::note(7), Step::note(9)],
        ])
        .with_one_shot(true);

    advance_gate(&mut seq); // cell 0 step 0

    // Request a jump straight to cell 2; it defers to the cycle end and must
    // win over the one_shot auto-advance to cell 1.
    seq.set_control("selected_sequence", 2.0).unwrap();
    advance_gate(&mut seq); // cell 0 step 1
    advance_gate(&mut seq); // cycle end: pending jump applies
    assert_eq!(seq.get_output("sequence").unwrap(), 2.0);
    assert_eq!(seq.get_output("end").unwrap(), 0.0);

    // Cell 2 is the last cell; the bank ends after it.
    advance_gate(&mut seq);
    advance_gate(&mut seq);
    assert_eq!(seq.get_output("end").unwrap(), 1.0);
}

#[test]
fn test_loop_mode_cell_end_never_fires() {
    let mut seq = CellSequencer::new(44_100)
        .with_steps(2)
        .with_sequences(vec![vec![Step::note(0), Step::note(2)]]);

    for _ in 0..8 {
        advance_gate(&mut seq);
        assert_eq!(seq.get_output("end").unwrap(), 0.0);
    }
    assert!(seq.ctrl.loop_count() > 0, "the cell keeps looping");
}

#[test]
fn test_cell_mode_control_round_trips() {
    let seq = CellSequencer::new(44_100);
    let meta = Module::controls(&seq);
    assert_eq!(meta.iter().filter(|c| c.key == "mode").count(), 1);

    assert_eq!(seq.ctrl.mode(), "loop");
    seq.ctrl
        .set_control("mode", ControlValue::from("one_shot"))
        .unwrap();
    assert_eq!(seq.ctrl.mode(), "one_shot");
    assert!(seq.ctrl.set_mode("bounce").is_err());
    match seq.ctrl.get_control("mode").unwrap() {
        ControlValue::String(value) => assert_eq!(value, "one_shot"),
        other => panic!("expected string mode, got {:?}", other),
    }
}

#[test]
fn test_cells_hold_long_sequences() {
    // A full through-composed lane (e.g. a 604-step Flow-of-Water voice)
    // fits in one cell; MAX_STEPS bounds bank-swap cost, not playback.
    let long: Vec<Step> = (0..604).map(|i| Step::note((i % 12) as i8)).collect();
    let mut seq = CellSequencer::new(44_100)
        .with_steps(604)
        .with_sequences(vec![long])
        .with_one_shot(true);

    for _ in 0..604 {
        advance_gate(&mut seq);
        assert_eq!(seq.get_output("end").unwrap(), 0.0);
    }
    advance_gate(&mut seq);
    assert_eq!(seq.get_output("end").unwrap(), 1.0, "ends after 604 steps");
}

#[test]
fn test_cell_sequencer_velocity_follows_step_amplitude() {
    let mut soft = Step::note(0);
    soft.amplitude = Some(0.25);
    let mut seq = CellSequencer::new(10).with_steps(4).with_sequences(vec![vec![
        soft,
        Step::held(),
        Step::rest(),
        Step::note(7), // No amplitude: velocity returns to full.
    ]]);

    assert_eq!(seq.get_output("velocity").unwrap(), 1.0);

    advance_gate(&mut seq);
    assert_eq!(seq.get_output("velocity").unwrap(), 0.25);

    // Holds, rests, and releases keep the struck velocity: a ringing tail
    // must never see its level jump.
    advance_gate(&mut seq);
    assert_eq!(seq.get_output("velocity").unwrap(), 0.25);
    advance_gate(&mut seq);
    assert_eq!(seq.get_output("velocity").unwrap(), 0.25);

    advance_gate(&mut seq);
    assert_eq!(seq.get_output("velocity").unwrap(), 1.0);
}

/// FUG-188 regression: the very first step's duration is a default estimate
/// (sample_rate / 2) until the second clock edge measures the real one. At a
/// faster tempo the opening note's gate would overrun the whole first step
/// and swallow the retrigger of a note on step 1 — the sequencer must force
/// a one-sample release edge so consecutive opening notes both strike.
#[test]
fn test_first_step_overrun_still_retriggers_next_note() {
    // sample_rate 1000 -> default step estimate 500 samples; the actual
    // clock runs a step every 100 samples, so the first gate (0.95 * 500)
    // would otherwise stay high straight through the second onset.
    let mut seq = CellSequencer::new(1000)
        .with_steps(4)
        .with_gate_length(0.95)
        .with_sequences(vec![vec![
            Step::note(0),
            Step::note(5),
            Step::rest(),
            Step::rest(),
        ]]);

    let mut rising_edges = 0;
    let mut last_gate = 0.0;
    for sample in 0..400 {
        let clock = if sample % 100 < 50 { 1.0 } else { 0.0 };
        seq.set_input("gate", clock).unwrap();
        seq.process(1);
        let gate = seq.get_output("gate").unwrap();
        if gate > 0.5 && last_gate <= 0.5 {
            rising_edges += 1;
        }
        last_gate = gate;
    }

    assert_eq!(
        rising_edges, 2,
        "both opening notes must produce a rising edge even though the \
         first step's duration was over-estimated"
    );
}
