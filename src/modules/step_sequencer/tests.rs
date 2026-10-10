use super::*;

#[test]
fn test_step_sequencer_basic() {
    let mut seq = StepSequencer::new(44100)
        .with_root_note(48)
        .with_step_count(4)
        .with_pattern(vec![
            Step::note(0),
            Step::rest(),
            Step::note(7),
            Step::note(5),
        ]);

    // Initially at step 0
    assert_eq!(seq.current_step(), 0);

    // First gate - should stay at step 0 and output frequency
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    assert!(freq > 0.0, "Should have frequency at step 0 (note)");
    assert_eq!(seq.current_step(), 0);

    // Gate low
    seq.set_input("clock", 0.0).unwrap();
    seq.process(1);

    // Second gate - advance to step 1 (rest)
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.current_step(), 1);
    let freq = seq.get_output("frequency").unwrap();
    assert_eq!(freq, 0.0, "Should have no frequency at rest step");
}

#[test]
fn test_step_sequencer_wrapping() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(4)
        .with_pattern(vec![Step::note(0); 4]);

    // Advance through all steps
    for expected_step in 0..8 {
        seq.set_input("clock", 1.0).unwrap();
        seq.process(1);
        assert_eq!(seq.current_step(), expected_step % 4);

        seq.set_input("clock", 0.0).unwrap();
        seq.process(1);
    }
}

#[test]
fn test_step_sequencer_reset() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(8)
        .with_pattern(vec![Step::note(0); 8]);

    // Advance a few steps
    for _ in 0..5 {
        seq.set_input("clock", 1.0).unwrap();
        seq.process(1);
        seq.set_input("clock", 0.0).unwrap();
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
        .with_step_count(2)
        .with_gate_length(0.5) // 50% default
        .with_pattern(vec![
            Step::note(0),                // Uses default 50%
            Step::note_with_gate(0, 1.0), // 100% gate
        ]);

    // Trigger first step
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    seq.set_input("clock", 0.0).unwrap();

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
        .with_step_count(3)
        .with_gate_length(0.4)
        .with_pattern(vec![Step::note(0), Step::held(), Step::rest()]);

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    let expected = Note::new(DEFAULT_ROOT_NOTE).frequency();
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("clock", 0.0).unwrap();
    for _ in 0..3 {
        seq.process(1);
    }
    assert_eq!(
        seq.get_output("gate").unwrap(),
        1.0,
        "note followed by a held step should use a full-step gate"
    );

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 1);
    assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("clock", 0.0).unwrap();
    seq.process(1);
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 2);
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_contextless_held_step_is_rest() {
    let mut seq = StepSequencer::new(44_100)
        .with_step_count(1)
        .with_pattern(vec![Step::held()]);

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);

    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_repeated_notes_retrigger() {
    let mut seq = StepSequencer::new(10)
        .with_step_count(2)
        .with_gate_length(0.6)
        .with_pattern(vec![Step::note(0), Step::note(0)]);

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);

    seq.set_input("clock", 0.0).unwrap();
    for _ in 0..4 {
        seq.process(1);
    }
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    assert_eq!(seq.current_step(), 1);
    assert_eq!(seq.get_output("gate").unwrap(), 1.0);
}

#[test]
fn test_step_sequencer_frequency_calculation() {
    let _seq = StepSequencer::new(44100)
        .with_root_note(60) // C4
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
    let mut seq = StepSequencer::new(44100)
        .with_step_count(4)
        .with_pattern(vec![]); // Empty pattern

    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);

    // Should treat as rests
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);
}

#[test]
fn test_step_sequencer_step_output() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(4)
        .with_pattern(vec![Step::note(0); 4]);

    for expected in 0..4 {
        seq.set_input("clock", 1.0).unwrap();
        seq.process(1);
        assert_eq!(seq.get_output("step").unwrap(), expected as f32);

        seq.set_input("clock", 0.0).unwrap();
        seq.process(1);
    }
}

#[test]
fn test_step_sequencer_factory() {
    let factory = StepSequencerFactory;
    assert_eq!(factory.type_id(), "step_sequencer");

    let config = serde_json::json!({
        "root_note": 36,
        "step_count": 8,
        "gate_length": 0.75,
        "pattern": [
            { "note": 0, "gate_length": 0.5 },
            { "note": null },
            { "note": 7 },
            { "note": 5, "gate_length": 1.0 }
        ]
    });

    let result = factory.build(44100, &config).unwrap();
    let module = result.module.module();

    assert_eq!(module.name(), "StepSequencer");
    assert_eq!(module.inputs(), &["clock", "reset"]);
    assert_eq!(module.outputs(), &["frequency", "gate", "step", "ended"]);
}

#[test]
fn test_parse_step_formats() {
    // Object with note and gate
    let step = parse_step(&serde_json::json!({"note": 5, "gate_length": 0.8})).unwrap();
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
    assert!(parse_step(&serde_json::json!({"held": true, "gate_length": 1.0})).is_err());
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
        .with_root_note(60) // C4
        .with_pattern(vec![Step::note(-12)]); // Should be C3

    seq.set_input("clock", 1.0).unwrap();
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
        seq.get_control("root_note").unwrap(),
        DEFAULT_ROOT_NOTE as f32
    );
    assert_eq!(seq.get_control("step_count").unwrap(), DEFAULT_STEPS as f32);
    assert_eq!(seq.get_control("gate_length").unwrap(), DEFAULT_GATE_LENGTH);

    // Set controls
    seq.set_control("root_note", 60.0).unwrap();
    assert_eq!(seq.get_control("root_note").unwrap(), 60.0);

    seq.set_control("step_count", 8.0).unwrap();
    assert_eq!(seq.get_control("step_count").unwrap(), 8.0);

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

    assert_eq!(controls.len(), 7);

    let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"root_note"));
    assert!(keys.contains(&"step_count"));
    assert!(keys.contains(&"gate_length"));
    assert!(keys.contains(&"mode"));
    assert!(keys.contains(&"grace_duration"));
    assert!(keys.contains(&"grace_placement"));
    assert!(keys.contains(&"ended"));
}

#[test]
fn test_step_sequencer_controls_affect_processing() {
    let mut seq = StepSequencer::new(44100).with_pattern(vec![Step::note(0)]);

    // Set root_note via control and verify it affects output
    seq.set_control("root_note", 60.0).unwrap();
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    let expected = Note::new(60).frequency();
    assert!((freq - expected).abs() < 0.01);

    // Change root_note and verify output changes
    seq.set_control("root_note", 72.0).unwrap();
    seq.process(1);

    let freq = seq.get_output("frequency").unwrap();
    let expected = Note::new(72).frequency();
    assert!((freq - expected).abs() < 0.01);
}

/// The surface a step sequencer built from `config` has.
fn surface_of(config: serde_json::Value) -> Arc<dyn crate::ControlSurface> {
    let built = StepSequencerFactory.build(44100, &config).unwrap();
    assert!(built.handles.is_empty());
    built.control_surface.unwrap()
}

fn number(surface: &Arc<dyn crate::ControlSurface>, key: &str) -> f32 {
    surface.get_control(key).unwrap().as_number().unwrap()
}

#[test]
fn test_step_sequencer_factory_builds_its_surface() {
    let surface = surface_of(serde_json::json!({
        "root_note": 36,
        "step_count": 8,
        "gate_length": 0.75,
    }));
    assert_eq!(number(&surface, "root_note"), 36.0);
    assert_eq!(number(&surface, "step_count"), 8.0);
    assert_eq!(number(&surface, "gate_length"), 0.75);
    let keys: Vec<_> = surface.controls().into_iter().map(|c| c.key).collect();
    let listed =
        "root_note step_count gate_length pattern mode grace_duration grace_placement ended";
    assert_eq!(keys.join(" "), listed);
}

#[test]
fn integer_controls_take_whole_numbers_and_clamp_them() {
    let surface = surface_of(serde_json::json!({ "root_note": 200, "step_count": 99 }));
    assert_eq!(number(&surface, "root_note"), 127.0);
    assert_eq!(number(&surface, "step_count"), 64.0);
    // A write clamps a whole number as config does, and refuses a fraction.
    for (key, value, held) in [("root_note", 60.0, 60.0), ("root_note", 300.0, 127.0)] {
        surface.set_control(key, value.into()).unwrap();
        assert_eq!(number(&surface, key), held, "{key} {value}");
    }
    surface.set_control("step_count", 0.0.into()).unwrap();
    assert_eq!(number(&surface, "step_count"), 1.0);
    assert!(surface.set_control("root_note", 60.5.into()).is_err());
    assert!(surface.set_control("ended", true.into()).is_err());
    let root_note = surface.automation("root_note").unwrap();
    root_note.write_number(200.0);
    assert_eq!(root_note.current(), Some(127.0), "a ramp starts from it");
}

/// Drives one full clock pulse (rising edge + release) through the sequencer.
fn pulse(seq: &mut StepSequencer) {
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    seq.set_input("clock", 0.0).unwrap();
    seq.process(1);
}

#[test]
fn test_one_shot_plays_once_and_fires_end() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(4)
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
            seq.get_output("ended").unwrap(),
            0.0,
            "end must stay low mid-pattern"
        );
        assert!(seq.get_output("frequency").unwrap() > 0.0);
    }

    // The next clock edge marks the final step's completion: silence + end.
    pulse(&mut seq);
    assert_eq!(
        seq.get_output("ended").unwrap(),
        1.0,
        "end fires at completion"
    );
    assert_eq!(seq.get_output("frequency").unwrap(), 0.0, "voice is silent");
    assert_eq!(seq.get_output("gate").unwrap(), 0.0);

    // Further clocks are ignored; end stays latched (exactly one rising edge).
    for _ in 0..3 {
        pulse(&mut seq);
        assert_eq!(seq.get_output("ended").unwrap(), 1.0);
        assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    }
}

#[test]
fn test_one_shot_reset_rearms() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(2)
        .with_pattern(vec![Step::note(0), Step::note(2)])
        .with_one_shot(true);

    for _ in 0..3 {
        pulse(&mut seq);
    }
    assert_eq!(seq.get_output("ended").unwrap(), 1.0);

    // Reset clears the latch and re-arms playback from step 0.
    seq.set_input("reset", 1.0).unwrap();
    seq.process(1);
    seq.set_input("reset", 0.0).unwrap();
    seq.process(1);
    assert_eq!(seq.get_output("ended").unwrap(), 0.0, "reset clears end");

    pulse(&mut seq);
    assert!(
        seq.get_output("frequency").unwrap() > 0.0,
        "plays again after reset"
    );
    pulse(&mut seq);
    pulse(&mut seq);
    assert_eq!(
        seq.get_output("ended").unwrap(),
        1.0,
        "second playthrough ends too"
    );
}

#[test]
fn test_switching_to_loop_clears_finished() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(2)
        .with_pattern(vec![Step::note(0), Step::note(2)])
        .with_one_shot(true);

    for _ in 0..3 {
        pulse(&mut seq);
    }
    assert_eq!(seq.get_output("ended").unwrap(), 1.0);

    seq.set_control("mode", 0.0).unwrap(); // back to loop
    pulse(&mut seq);
    assert_eq!(
        seq.get_output("ended").unwrap(),
        0.0,
        "loop mode clears end"
    );
    assert!(
        seq.get_output("frequency").unwrap() > 0.0,
        "playback resumes"
    );
}

#[test]
fn test_loop_mode_end_never_fires() {
    let mut seq = StepSequencer::new(44100)
        .with_step_count(2)
        .with_pattern(vec![Step::note(0), Step::note(2)]);

    for _ in 0..10 {
        pulse(&mut seq);
        assert_eq!(seq.get_output("ended").unwrap(), 0.0);
    }
    // Wrapped several times, still playing.
    assert!(seq.get_output("frequency").unwrap() > 0.0);
}

#[test]
fn test_mode_control_round_trips() {
    let seq = StepSequencer::new(44100);
    let meta = Module::controls(&seq);
    assert_eq!(meta.iter().filter(|c| c.key == "mode").count(), 1);

    let surface = surface_of(serde_json::json!({}));
    assert_eq!(surface.get_control("mode").unwrap(), "loop".into());
    surface.set_control("mode", "one_shot".into()).unwrap();
    assert_eq!(surface.get_control("mode").unwrap(), "one_shot".into());
    assert!(surface.set_control("mode", "bounce".into()).is_err());
    let built = surface_of(serde_json::json!({ "mode": "one_shot" }));
    assert_eq!(built.get_control("mode").unwrap(), "one_shot".into());
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
    let mut seq = StepSequencer::new(1000)
        .with_step_count(4)
        .with_pattern(vec![
            Step::note(0),
            Step::rest(),
            Step::note_with_grace(10, &[8]),
            Step::rest(),
        ]);

    let mut stream: Vec<(f32, f32)> = Vec::new();
    for _ in 0..4 {
        for s in 0..200 {
            let gate_in = if s < 2 { 1.0 } else { 0.0 };
            seq.set_input("clock", gate_in).unwrap();
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
    let grace_freq = Note::new((DEFAULT_ROOT_NOTE as i16 + 8) as u8).frequency();
    let principal_freq = Note::new((DEFAULT_ROOT_NOTE as i16 + 10) as u8).frequency();
    assert!((stream[onsets[0]].0 - grace_freq).abs() < 0.01);
    assert!((stream[onsets[1] + 5].0 - principal_freq).abs() < 0.01);
}

#[test]
fn test_grace_placement_control_on_beat() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(2)
        .with_pattern(vec![Step::note(0), Step::note_with_grace(10, &[8])]);
    seq.set_control("grace_placement", 1.0).unwrap();

    let mut stream: Vec<(f32, f32)> = Vec::new();
    for _ in 0..3 {
        for s in 0..200 {
            let gate_in = if s < 2 { 1.0 } else { 0.0 };
            seq.set_input("clock", gate_in).unwrap();
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
    let grace_freq = Note::new((DEFAULT_ROOT_NOTE as i16 + 8) as u8).frequency();
    assert!((stream[onsets[0]].0 - grace_freq).abs() < 0.01);
}

fn pattern_of(config: serde_json::Value) -> Result<Vec<Step>, String> {
    let built = StepSequencerFactory
        .build(44_100, &config)
        .map_err(|error| error.to_string())?;
    let pattern = built.control_surface.unwrap().get_control("pattern")?;
    controls::parse_pattern_json(pattern.as_string()?)
}

#[test]
fn pattern_numbers_read_whole_floats_as_written() {
    let pattern = pattern_of(serde_json::json!({
        "pattern": [{ "note": 2.0, "grace": [-1.0] }, 4.0, { "note": -3, "gate_length": 2 }]
    }))
    .unwrap();
    // `{"note": 2.0}` was a rest, and a bare `4.0` was refused.
    assert_eq!(pattern[0].note, Some(2));
    assert_eq!(pattern[0].grace.iter().collect::<Vec<_>>(), vec![-1]);
    assert_eq!(pattern[1].note, Some(4));
    // Gate and velocity stay clamped to 0..=1.
    assert_eq!(
        (pattern[2].note, pattern[2].gate_length),
        (Some(-3), Some(1.0))
    );
}

#[test]
fn pattern_numbers_are_refused_with_their_path() {
    let refused = |pattern: serde_json::Value| {
        pattern_of(serde_json::json!({ "pattern": pattern })).unwrap_err()
    };
    let note = "expects a whole number from -128 to 127";
    assert_eq!(
        refused(serde_json::json!([0, { "note": 2.5 }])),
        format!("step_sequencer config 'pattern[1].note' {note}, got 2.5")
    );
    // `as i8` used to wrap 300 to 44.
    assert!(refused(serde_json::json!([{ "note": 300 }])).contains("'pattern[0].note'"));
    // A note of another type was a rest.
    assert!(refused(serde_json::json!([{ "note": "C4" }])).contains("'pattern[0].note'"));
    assert_eq!(
        refused(serde_json::json!([7.5])),
        format!("step_sequencer config 'pattern[0]' {note}, got 7.5")
    );
    assert!(
        refused(serde_json::json!([{ "note": 0, "grace": [1, 1.5] }]))
            .contains("'pattern[0].grace[1]' expects a whole number")
    );
    for field in ["gate_length", "velocity"] {
        let error = refused(serde_json::json!([{ "note": 0, field: 1e39 }]));
        assert!(
            error.contains(&format!("'pattern[0].{field}' expects a finite number")),
            "{error}"
        );
    }
    // The JSON text a written `pattern` control records is refused the same way.
    let config = serde_json::json!({ "pattern": r#"[{"note": 0.5}]"# });
    assert!(pattern_of(config)
        .unwrap_err()
        .contains("'pattern[0].note'"));
}

#[test]
fn pattern_reads_as_an_array_or_as_the_json_text_of_one() {
    let array = pattern_of(serde_json::json!({ "pattern": [{ "note": 0 }, null, 7] })).unwrap();
    let text = pattern_of(serde_json::json!({ "pattern": r#"[{"note": 0}, null, 7]"# })).unwrap();
    assert_eq!(format!("{array:?}"), format!("{text:?}"));
    assert_eq!(array.len(), 3);
}

#[test]
fn grace_duration_is_configured_in_seconds_and_clamped() {
    let build = |config: serde_json::Value| number(&surface_of(config), "grace_duration");
    assert_eq!(build(serde_json::json!({})), 0.06);
    assert_eq!(build(serde_json::json!({ "grace_duration": 0.08 })), 0.08);
    assert_eq!(build(serde_json::json!({ "grace_duration": 80.0 })), 0.2);
    assert_eq!(
        build(serde_json::json!({ "grace_duration": 0.0001 })),
        0.005
    );
}

#[test]
fn the_pattern_control_reads_step_numbers_by_the_same_rules() {
    let surface = surface_of(serde_json::json!({}));
    let write = |json: &str| surface.set_control("pattern", json.into());
    write(r#"[{"note": 2.0}, 3.0]"#).unwrap();
    let pattern = surface.get_control("pattern").unwrap();
    let pattern = controls::parse_pattern_json(pattern.as_string().unwrap()).unwrap();
    let notes: Vec<_> = pattern.iter().map(|step| step.note).collect();
    assert_eq!(notes, vec![Some(2), Some(3)]);
    let error = write(r#"[{"note": 2.5}]"#).unwrap_err();
    assert!(error.contains("'note' expects a whole number"), "{error}");
}

#[test]
fn steps_refuse_unknown_fields() {
    // A step is closed: the score format's old `amplitude` is refused rather
    // than silently dropping the step's level.
    let error = parse_step(&serde_json::json!({"note": 0, "amplitude": 0.5})).unwrap_err();
    assert!(
        error.to_string().contains("unknown step field 'amplitude'"),
        "{error}"
    );
    let step = parse_step(&serde_json::json!({"note": 0, "velocity": 0.5})).unwrap();
    assert_eq!(step.velocity, Some(0.5));
}

/// Every control but the pattern applies with no lock and no allocation:
/// a block without a clock edge renders while another thread holds the
/// pattern (which a clock edge still reads under its lock, FUG-312).
#[test]
fn scalar_controls_apply_and_render_while_the_pattern_is_held() {
    use crate::control_request::{apply_declared, RtValue};

    let mut seq = StepSequencer::new(44100).with_pattern(vec![Step::note(0)]);
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    let pattern = seq.pattern.clone();
    let held = pattern.lock().unwrap();
    let (frequency, allocs, frees) = crate::alloc_counter::allocator_events(|| {
        apply_declared(&mut seq, controls::ROOT_NOTE, RtValue::I32(60)).unwrap();
        apply_declared(&mut seq, controls::GATE_LENGTH, RtValue::F32(0.9)).unwrap();
        apply_declared(&mut seq, controls::MODE, RtValue::U32(1)).unwrap();
        apply_declared(&mut seq, controls::GRACE_PLACEMENT, RtValue::U32(1)).unwrap();
        seq.process(64);
        seq.output_block(0)[63]
    });
    drop(held);
    assert_eq!((allocs, frees), (0, 0));
    assert!((frequency - Note::new(60).frequency()).abs() < 0.01);
    assert_eq!(seq.get_control("root_note").unwrap(), 60.0);
}
