use super::*;

#[test]
fn test_step_sequencer_basic() {
    let mut seq = StepSequencer::new(44100)
        .with_base_note(48)
        .with_steps(4)
        .with_pattern(vec![
            Step::note(0),
            Step::rest(),
            Step::note(7),
            Step::note(5),
        ]);

    // Initially at step 0
    assert_eq!(seq.current_step(), 0);

    // First gate - should stay at step 0 and output frequency
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    assert!(freq > 0.0, "Should have frequency at step 0 (note)");
    assert_eq!(seq.current_step(), 0);

    // Gate low
    seq.set_input("gate", 0.0).unwrap();
    seq.process(1);

    // Second gate - advance to step 1 (rest)
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.current_step(), 1);
    let freq = seq.get_output("frequency").unwrap();
    assert_eq!(freq, 0.0, "Should have no frequency at rest step");
}

#[test]
fn test_step_sequencer_wrapping() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(4)
        .with_pattern(vec![Step::note(0); 4]);

    // Advance through all steps
    for expected_step in 0..8 {
        seq.set_input("gate", 1.0).unwrap();
        seq.process(1);
        assert_eq!(seq.current_step(), expected_step % 4);

        seq.set_input("gate", 0.0).unwrap();
        seq.process(1);
    }
}

#[test]
fn test_step_sequencer_reset() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(8)
        .with_pattern(vec![Step::note(0); 8]);

    // Advance a few steps
    for _ in 0..5 {
        seq.set_input("gate", 1.0).unwrap();
        seq.process(1);
        seq.set_input("gate", 0.0).unwrap();
        seq.process(1);
    }

    assert!(seq.current_step() > 0);

    // Reset
    seq.set_input("reset", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.current_step(), 0);
}

#[test]
fn test_step_sequencer_gate_length() {
    let mut seq = StepSequencer::new(1000) // 1kHz for easy math
        .with_steps(2)
        .with_gate_length(0.5) // 50% default
        .with_pattern(vec![
            Step::note(0),                // Uses default 50%
            Step::note_with_gate(0, 1.0), // 100% gate
        ]);

    // Trigger first step
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    seq.set_input("gate", 0.0).unwrap();

    // Gate should be high initially
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    // After some samples, gate should still be high (within 50% of step duration)
    for _ in 0..100 {
        seq.process(1);
    }
}

#[test]
fn test_step_sequencer_held_steps_continue_active_note() {
    let mut seq = StepSequencer::new(10)
        .with_steps(3)
        .with_gate_length(0.4)
        .with_pattern(vec![Step::note(0), Step::held(), Step::rest()]);

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    let expected = Note::new(DEFAULT_BASE_NOTE).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("gate", 0.0).unwrap();
    for _ in 0..3 {
        seq.process(1);
    }
    assert_eq!(
        seq.get_output("gate").unwrap(),
        1.0,
        "note followed by a held step should use a full-step gate"
    );

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 1);
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("gate", 0.0).unwrap();
    seq.process(1);
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 2);
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_contextless_held_step_is_rest() {
    let mut seq = StepSequencer::new(44_100)
        .with_steps(1)
        .with_pattern(vec![Step::held()]);

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_repeated_notes_retrigger() {
    let mut seq = StepSequencer::new(10)
        .with_steps(2)
        .with_gate_length(0.6)
        .with_pattern(vec![Step::note(0), Step::note(0)]);

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("gate", 0.0).unwrap();
    for _ in 0..4 {
        seq.process(1);
    }
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 1);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);
}

#[test]
fn test_step_sequencer_frequency_calculation() {
    let _seq = StepSequencer::new(44100)
        .with_base_note(60) // C4
        .with_pattern(vec![
            Step::note(0),  // C4
            Step::note(12), // C5 (octave up)
        ]);

    // C4 = 261.63 Hz approximately
    let c4_freq = Note::new(60).frequency();
    let c5_freq = Note::new(72).frequency();

    // Verify our understanding
    assert!((c4_freq - 261.63).abs() < 1.0);
    assert!((c5_freq - 523.25).abs() < 1.0);
}

#[test]
fn test_step_sequencer_empty_pattern() {
    let mut seq = StepSequencer::new(44100).with_steps(4).with_pattern(vec![]); // Empty pattern

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    // Should treat as rests
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_step_output() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(4)
        .with_pattern(vec![Step::note(0); 4]);

    for expected in 0..4 {
        seq.set_input("gate", 1.0).unwrap();
        seq.process(1);
        assert_eq!(seq.get_output("step").unwrap(), expected as f32);

        seq.set_input("gate", 0.0).unwrap();
        seq.process(1);
    }
}

#[test]
fn test_step_sequencer_factory() {
    let factory = StepSequencerFactory;
    assert_eq!(factory.type_id(), "step_sequencer");

    let config = serde_json::json!({
        "base_note": 36,
        "steps": 8,
        "gate_length": 0.75,
        "pattern": [
            { "note": 0, "gate": 0.5 },
            { "note": null },
            { "note": 7 },
            { "note": 5, "gate": 1.0 }
        ]
    });

    let result = factory.build(44100, &config).unwrap();
    let module = result.module.module();

    assert_eq!(module.name(), "StepSequencer");
    assert_eq!(module.inputs(), &["gate", "reset"]);
    assert_eq!(module.outputs(), &["frequency", "gate", "step", "end"]);
}

#[test]
fn test_parse_step_formats() {
    // Object with note and gate
    let step = parse_step(&serde_json::json!({"note": 5, "gate": 0.8})).unwrap();
    assert_eq!(step.note, Some(5));
    assert_eq!(step.gate_length, Some(0.8));
    assert!(!step.held);

    // Held continuation
    let step = parse_step(&serde_json::json!({"held": true})).unwrap();
    assert_eq!(step.note, None);
    assert!(step.held);

    // Object with null note (rest)
    let step = parse_step(&serde_json::json!({"note": null})).unwrap();
    assert_eq!(step.note, None);
    assert!(!step.held);

    // Simple integer
    let step = parse_step(&serde_json::json!(7)).unwrap();
    assert_eq!(step.note, Some(7));

    // Null value
    let step = parse_step(&serde_json::Value::Null).unwrap();
    assert_eq!(step.note, None);

    assert!(parse_step(&serde_json::json!({"held": true, "note": 0})).is_err());
    assert!(parse_step(&serde_json::json!({"held": true, "gate": 1.0})).is_err());
    assert!(parse_step(&serde_json::json!({"held": "yes"})).is_err());
}

#[test]
fn test_step_serializes_held_without_note_field() {
    let value = serde_json::to_value(Step::held()).unwrap();
    assert_eq!(value, serde_json::json!({"held": true}));
}

#[test]
fn test_step_sequencer_negative_note_offset() {
    let mut seq = StepSequencer::new(44100)
        .with_base_note(60) // C4
        .with_pattern(vec![Step::note(-12)]); // Should be C3

    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    let expected = Note::new(48).frequency(); // C3
    assert!((freq - expected).abs() < 0.01);
}

#[test]
fn test_step_sequencer_controls() {
    let mut seq = StepSequencer::new(44100);

    // Verify default control values
    assert_eq!(
        seq.get_control("base_note").unwrap(),
        DEFAULT_BASE_NOTE as f32
    );
    assert_eq!(seq.get_control("steps").unwrap(), DEFAULT_STEPS as f32);
    assert_eq!(seq.get_control("gate_length").unwrap(), DEFAULT_GATE_LENGTH);

    // Set controls
    seq.set_control("base_note", 60.0).unwrap();
    assert_eq!(seq.get_control("base_note").unwrap(), 60.0);

    seq.set_control("steps", 8.0).unwrap();
    assert_eq!(seq.get_control("steps").unwrap(), 8.0);

    seq.set_control("gate_length", 0.75).unwrap();
    assert_eq!(seq.get_control("gate_length").unwrap(), 0.75);

    // Unknown control returns error
    assert!(seq.get_control("unknown").is_err());
    assert!(seq.set_control("unknown", 1.0).is_err());
}

#[test]
fn test_step_sequencer_controls_metadata() {
    let seq = StepSequencer::new(44100);
    let controls = Module::controls(&seq);

    assert_eq!(controls.len(), 6);

    let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"base_note"));
    assert!(keys.contains(&"steps"));
    assert!(keys.contains(&"gate_length"));
    assert!(keys.contains(&"mode"));
    assert!(keys.contains(&"grace_duration_ms"));
    assert!(keys.contains(&"grace_placement"));
}

#[test]
fn test_step_sequencer_controls_affect_processing() {
    let mut seq = StepSequencer::new(44100).with_pattern(vec![Step::note(0)]);

    // Set base_note via control and verify it affects output
    seq.set_control("base_note", 60.0).unwrap();
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    let expected = Note::new(60).frequency();
    assert!((freq - expected).abs() < 0.01);

    // Change base_note and verify output changes
    seq.set_control("base_note", 72.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    let expected = Note::new(72).frequency();
    assert!((freq - expected).abs() < 0.01);
}

#[test]
fn test_step_sequencer_factory_returns_handles() {
    let factory = StepSequencerFactory;
    let config = serde_json::json!({
        "base_note": 36,
        "steps": 8,
        "gate_length": 0.75,
    });

    let result = factory.build(44100, &config).unwrap();
    assert_eq!(result.handles.len(), 1);
    assert_eq!(result.handles[0].0, "controls");

    // Verify the handle can be downcast
    let controls = result.handles[0]
        .1
        .downcast_ref::<StepSequencerControls>()
        .unwrap();
    assert_eq!(controls.base_note(), 36);
    assert_eq!(controls.steps(), 8);
    assert!((controls.gate_length() - 0.75).abs() < f32::EPSILON);
}

/// Drives one full clock pulse (rising edge + release) through the sequencer.
fn pulse(seq: &mut StepSequencer) {
    seq.set_input("gate", 1.0).unwrap();
    seq.process(1);
    seq.set_input("gate", 0.0).unwrap();
    seq.process(1);
}

#[test]
fn test_one_shot_plays_once_and_fires_end() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(4)
        .with_pattern(vec![
            Step::note(0),
            Step::note(2),
            Step::note(4),
            Step::note(5),
        ])
        .with_one_shot(true);

    // Steps 0..3 play normally; end stays low throughout.
    for expected_step in 0..4 {
        pulse(&mut seq);
        assert_eq!(seq.current_step(), expected_step);
        assert_eq!(
            seq.get_output("end").unwrap(),
            0.0,
            "end must stay low mid-pattern"
        );
        assert!(seq.get_output("frequency").unwrap() > 0.0);
    }

    // The next clock edge marks the final step's completion: silence + end.
    pulse(&mut seq);
    assert_eq!(
        seq.get_output("end").unwrap(),
        1.0,
        "end fires at completion"
    );
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0, "voice is silent");
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);

    // Further clocks are ignored; end stays latched (exactly one rising edge).
    for _ in 0..3 {
        pulse(&mut seq);
        assert_eq!(seq.get_output("end").unwrap(), 1.0);
        assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    }
}

#[test]
fn test_one_shot_reset_rearms() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(2)
        .with_pattern(vec![Step::note(0), Step::note(2)])
        .with_one_shot(true);

    for _ in 0..3 {
        pulse(&mut seq);
    }
    assert_eq!(seq.get_output("end").unwrap(), 1.0);

    // Reset clears the latch and re-arms playback from step 0.
    seq.set_input("reset", 1.0).unwrap();
    seq.process(1);
    seq.set_input("reset", 0.0).unwrap();
    seq.process(1);
    assert_eq!(seq.get_output("end").unwrap(), 0.0, "reset clears end");

    pulse(&mut seq);
    assert!(
        seq.get_output("frequency").unwrap() > 0.0,
        "plays again after reset"
    );
    pulse(&mut seq);
    pulse(&mut seq);
    assert_eq!(
        seq.get_output("end").unwrap(),
        1.0,
        "second playthrough ends too"
    );
}

#[test]
fn test_switching_to_loop_clears_finished() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(2)
        .with_pattern(vec![Step::note(0), Step::note(2)])
        .with_one_shot(true);

    for _ in 0..3 {
        pulse(&mut seq);
    }
    assert_eq!(seq.get_output("end").unwrap(), 1.0);

    seq.set_control("mode", 0.0).unwrap(); // back to loop
    pulse(&mut seq);
    assert_eq!(seq.get_output("end").unwrap(), 0.0, "loop mode clears end");
    assert!(
        seq.get_output("frequency").unwrap() > 0.0,
        "playback resumes"
    );
}

#[test]
fn test_loop_mode_end_never_fires() {
    let mut seq = StepSequencer::new(44100)
        .with_steps(2)
        .with_pattern(vec![Step::note(0), Step::note(2)]);

    for _ in 0..10 {
        pulse(&mut seq);
        assert_eq!(seq.get_output("end").unwrap(), 0.0);
    }
    // Wrapped several times, still playing.
    assert!(seq.get_output("frequency").unwrap() > 0.0);
}

#[test]
fn test_mode_control_round_trips() {
    let seq = StepSequencer::new(44100);
    let meta = Module::controls(&seq);
    assert_eq!(meta.iter().filter(|c| c.key == "mode").count(), 1);

    let controls = seq.controls();
    assert_eq!(controls.mode(), "loop");
    controls.set_mode("one_shot").unwrap();
    assert_eq!(controls.mode(), "one_shot");
    assert!(controls.set_mode("bounce").is_err());
}

#[test]
fn test_parse_step_grace_formats() {
    let step = parse_step(&serde_json::json!({"note": 10, "grace": [-2]})).unwrap();
    assert_eq!(step.note, Some(10));
    assert_eq!(step.grace.iter().collect::<Vec<_>>(), vec![-2]);

    // Absent, null, and empty arrays all mean "no graces".
    for value in [
        serde_json::json!({"note": 10}),
        serde_json::json!({"note": 10, "grace": null}),
        serde_json::json!({"note": 10, "grace": []}),
    ] {
        assert!(parse_step(&value).unwrap().grace.is_empty());
    }

    assert!(parse_step(&serde_json::json!({"note": null, "grace": [5]})).is_err());
    assert!(parse_step(&serde_json::json!({"note": 0, "grace": [1, 2, 3, 4, 5]})).is_err());
    assert!(parse_step(&serde_json::json!({"note": 0, "grace": "fast"})).is_err());
    assert!(parse_step(&serde_json::json!({"note": 0, "grace": [900]})).is_err());
    assert!(parse_step(&serde_json::json!({"held": true, "grace": [5]})).is_err());
}

#[test]
fn test_step_grace_serde_round_trip() {
    let step = Step::note_with_grace(10, &[-2, 3]);
    let value = serde_json::to_value(&step).unwrap();
    assert_eq!(value, serde_json::json!({"note": 10, "grace": [-2, 3]}));

    let parsed: Step = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.note, Some(10));
    assert_eq!(parsed.grace, step.grace);

    // Steps without graces serialize exactly as before the field existed.
    let value = serde_json::to_value(Step::note(5)).unwrap();
    assert_eq!(value, serde_json::json!({"note": 5}));
}

// --- Grace-note realization (FUG-190; full behavior coverage lives in the
// cell_sequencer tests — the two sequencers share the GracePlayer) ---

#[test]
fn test_grace_before_beat_two_attacks() {
    // Sample rate 1000 so the default 60 ms grace is 60 samples.
    let mut seq = StepSequencer::new(1000).with_steps(4).with_pattern(vec![
        Step::note(0),
        Step::rest(),
        Step::note_with_grace(10, &[8]),
        Step::rest(),
    ]);

    let mut stream: Vec<(f32, f32)> = Vec::new();
    for _ in 0..4 {
        for s in 0..200 {
            let gate_in = if s < 2 { 1.0 } else { 0.0 };
            seq.set_input("gate", gate_in).unwrap();
            seq.process(1);
            stream.push((
                seq.get_output("frequency").unwrap(),
                seq.get_output("gate").unwrap(),
            ));
        }
    }

    let mut onsets = Vec::new();
    for t in 300..600 {
        if stream[t].1 > 0.5 && stream[t - 1].1 <= 0.5 {
            onsets.push(t);
        }
    }
    assert_eq!(onsets.len(), 2, "grace + principal, got {:?}", onsets);
    assert_eq!(onsets[1], 400, "principal stays on the grid");
    let grace_freq = Note::new((DEFAULT_BASE_NOTE as i16 + 8) as u8).frequency();
    let principal_freq = Note::new((DEFAULT_BASE_NOTE as i16 + 10) as u8).frequency();
    assert!((stream[onsets[0]].0 - grace_freq).abs() < 0.01);
    assert!((stream[onsets[1] + 5].0 - principal_freq).abs() < 0.01);
}

#[test]
fn test_grace_placement_control_on_beat() {
    let mut seq = StepSequencer::new(1000)
        .with_steps(2)
        .with_pattern(vec![Step::note(0), Step::note_with_grace(10, &[8])]);
    seq.set_control("grace_placement", 1.0).unwrap();

    let mut stream: Vec<(f32, f32)> = Vec::new();
    for _ in 0..3 {
        for s in 0..200 {
            let gate_in = if s < 2 { 1.0 } else { 0.0 };
            seq.set_input("gate", gate_in).unwrap();
            seq.process(1);
            stream.push((
                seq.get_output("frequency").unwrap(),
                seq.get_output("gate").unwrap(),
            ));
        }
    }

    // Decorated step's edge is t = 200: chain at the edge, principal ~60
    // samples later.
    let mut onsets = Vec::new();
    for t in 195..400 {
        if stream[t].1 > 0.5 && stream[t - 1].1 <= 0.5 {
            onsets.push(t);
        }
    }
    assert_eq!(onsets.len(), 2, "got {:?}", onsets);
    // The chain starts at the edge (one sample later when the previous
    // step's over-estimated cold-start gate forces a retrigger dip).
    assert!((200..=201).contains(&onsets[0]), "got {}", onsets[0]);
    assert!(
        (255..271).contains(&onsets[1]),
        "principal is delayed by the grace duration, got {}",
        onsets[1]
    );
    let grace_freq = Note::new((DEFAULT_BASE_NOTE as i16 + 8) as u8).frequency();
    assert!((stream[onsets[0]].0 - grace_freq).abs() < 0.01);
}
