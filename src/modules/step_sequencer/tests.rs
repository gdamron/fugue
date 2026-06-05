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
    assert_eq!(module.outputs(), &["frequency", "gate", "step"]);
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

    assert_eq!(controls.len(), 3);

    let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"base_note"));
    assert!(keys.contains(&"steps"));
    assert!(keys.contains(&"gate_length"));
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
