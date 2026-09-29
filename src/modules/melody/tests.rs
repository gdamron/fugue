use super::*;

fn make_melody() -> MelodyGenerator {
    let controls = MelodyControls::new(60, vec![0, 2, 3, 5, 7, 9, 10]);
    MelodyGenerator::new(controls)
}

#[test]
fn test_melody_controls_metadata() {
    let melody = make_melody();
    let controls = Module::controls(&melody);

    // root_note + degree_count + 7 degrees + 7 weights = 16
    // (the seed control lives on the ControlSurface, not this legacy list)
    assert_eq!(controls.len(), 16);

    let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"root_note"));
    assert!(keys.contains(&"degree_count"));
    assert!(keys.contains(&"degree.0"));
    assert!(keys.contains(&"degree.6"));
    assert!(keys.contains(&"note_weight.0"));
    assert!(keys.contains(&"note_weight.6"));
}

#[test]
fn test_melody_get_degree_controls() {
    let melody = make_melody();

    assert_eq!(melody.get_control("degree_count").unwrap(), 7.0);
    assert_eq!(melody.get_control("degree.0").unwrap(), 0.0);
    assert_eq!(melody.get_control("degree.3").unwrap(), 5.0);
    assert_eq!(melody.get_control("note_weight.0").unwrap(), 1.0);
}

#[test]
fn test_melody_set_degree_controls() {
    let mut melody = make_melody();

    // Set a specific degree
    melody.set_control("degree.2", 5.0).unwrap();
    assert_eq!(melody.get_control("degree.2").unwrap(), 5.0);

    // Set a weight
    melody.set_control("note_weight.1", 3.5).unwrap();
    assert_eq!(melody.get_control("note_weight.1").unwrap(), 3.5);
}

#[test]
fn test_melody_degree_count_grow() {
    let mut melody = make_melody();

    melody.set_control("degree_count", 9.0).unwrap();
    assert_eq!(melody.get_control("degree_count").unwrap(), 9.0);

    // New degrees should be sequential after last existing degree (10)
    assert_eq!(melody.get_control("degree.7").unwrap(), 11.0);
    assert_eq!(melody.get_control("degree.8").unwrap(), 12.0);

    // New weights default to 1.0
    assert_eq!(melody.get_control("note_weight.7").unwrap(), 1.0);
    assert_eq!(melody.get_control("note_weight.8").unwrap(), 1.0);

    // Controls metadata should reflect new count
    let controls = Module::controls(&melody);
    // root_note + degree_count + 9 degrees + 9 weights = 20
    assert_eq!(controls.len(), 20);
}

#[test]
fn test_melody_degree_count_shrink() {
    let mut melody = make_melody();

    melody.set_control("degree_count", 3.0).unwrap();
    assert_eq!(melody.get_control("degree_count").unwrap(), 3.0);

    // Accessing beyond the new count should error
    assert!(melody.get_control("degree.3").is_err());
    assert!(melody.get_control("note_weight.3").is_err());
}

#[test]
fn test_melody_out_of_range_degree_errors() {
    let melody = make_melody();

    assert!(melody.get_control("degree.7").is_err());
    assert!(melody.get_control("note_weight.7").is_err());
}

#[test]
fn test_melody_unknown_control_errors() {
    let melody = make_melody();

    assert!(melody.get_control("unknown").is_err());
}

#[test]
fn test_melody_negative_degrees() {
    let controls = MelodyControls::new(60, vec![-2, 0, 2, 5]);
    let melody = MelodyGenerator::new(controls);

    assert_eq!(melody.get_control("degree.0").unwrap(), -2.0);
    assert_eq!(melody.get_control("degree.1").unwrap(), 0.0);
    assert_eq!(melody.get_control("degree.2").unwrap(), 2.0);
    assert_eq!(melody.get_control("degree.3").unwrap(), 5.0);
}

#[test]
fn test_melody_set_negative_degree() {
    let mut melody = make_melody();

    melody.set_control("degree.0", -3.0).unwrap();
    assert_eq!(melody.get_control("degree.0").unwrap(), -3.0);
}

/// Drives one gate pulse and returns the resulting frequency.
fn pulse_note(melody: &mut MelodyGenerator) -> f32 {
    melody.set_input("gate", 1.0).unwrap();
    melody.process(1);
    melody.set_input("gate", 0.0).unwrap();
    melody.process(1);
    melody.get_output("frequency").unwrap()
}

fn notes(melody: &mut MelodyGenerator, count: usize) -> Vec<f32> {
    (0..count).map(|_| pulse_note(melody)).collect()
}

fn seeded_melody(seed: u64) -> MelodyGenerator {
    let controls = MelodyControls::new(60, vec![0, 2, 3, 5, 7, 9, 10]);
    controls.set_seed(seed);
    MelodyGenerator::new(controls)
}

#[test]
fn test_same_seed_produces_identical_melodies() {
    let a = notes(&mut seeded_melody(42), 32);
    let b = notes(&mut seeded_melody(42), 32);
    assert_eq!(a, b, "same seed must reproduce the same note stream");

    let c = notes(&mut seeded_melody(43), 32);
    assert_ne!(a, c, "different seeds should diverge");
}

#[test]
fn test_reseeding_restarts_the_stream() {
    let mut melody = seeded_melody(7);
    let first = notes(&mut melody, 8);

    // Setting the same seed again restarts the stream from the top.
    melody.controls().set_seed(7);
    let replay = notes(&mut melody, 8);
    assert_eq!(first, replay);
}

#[test]
fn test_seed_control_surface_round_trip() {
    use crate::{ControlSurface, ControlValue};
    let controls = MelodyControls::new(60, vec![0, 2, 4]);
    assert_eq!(controls.seed(), None, "unseeded by default");

    controls
        .set_control("seed", ControlValue::Number(1234.0))
        .unwrap();
    assert_eq!(controls.seed(), Some(1234));
    assert_eq!(
        controls.get_control("seed").unwrap(),
        ControlValue::Number(1234.0)
    );
    assert!(controls.controls().iter().any(|meta| meta.key == "seed"));
}

#[test]
fn test_next_note_does_not_block_on_held_control_lock() {
    let mut melody = seeded_melody(5);
    melody.controls().set_allowed_degrees(vec![7]);

    // A control thread mid-edit holds the table lock. Previously this
    // deadlocked the audio thread; now the stale degrees are used.
    let controls = melody.controls().clone();
    let guard = controls.table.lock().unwrap();
    let stale = melody.next_note();
    assert_ne!(stale.midi_note, 67, "stale table has no degree 7");
    drop(guard);

    assert_eq!(melody.next_note().midi_note, 67);
}

#[test]
fn test_live_degree_and_root_changes_reach_the_audio_path() {
    let mut melody = seeded_melody(9);
    melody.controls().set_allowed_degrees(vec![0, 4]);
    melody.controls().set_note_weights(vec![0.0, 1.0]);
    assert_eq!(pulse_note(&mut melody), Note::new(64).frequency());

    melody.set_control("root_note", 62.0).unwrap();
    assert_eq!(pulse_note(&mut melody), Note::new(66).frequency());

    melody.set_control("note_weight.0", 1.0).unwrap();
    melody.set_control("note_weight.1", 0.0).unwrap();
    assert_eq!(pulse_note(&mut melody), Note::new(62).frequency());
}

#[test]
fn test_no_degrees_yields_middle_c() {
    let mut melody = MelodyGenerator::new(MelodyControls::new(67, vec![]));
    assert_eq!(melody.next_note().midi_note, 60);
}

#[test]
fn test_factory_seed_config() {
    use crate::factory::ModuleFactory;
    let factory = MelodyFactory;
    let build = |seed: u64| {
        let config = serde_json::json!({
            "root_note": 60,
            "scale_degrees": [0, 2, 4, 5, 7],
            "seed": seed
        });
        factory.build(48_000, &config).unwrap()
    };
    let mut first = build(99);
    let mut second = build(99);
    let a: Vec<f32> = (0..16)
        .map(|_| {
            first.module.module_mut().set_input("gate", 1.0).unwrap();
            first.module.module_mut().process(1);
            first.module.module_mut().set_input("gate", 0.0).unwrap();
            first.module.module_mut().process(1);
            first.module.module().get_output("frequency").unwrap()
        })
        .collect();
    let b: Vec<f32> = (0..16)
        .map(|_| {
            second.module.module_mut().set_input("gate", 1.0).unwrap();
            second.module.module_mut().process(1);
            second.module.module_mut().set_input("gate", 0.0).unwrap();
            second.module.module_mut().process(1);
            second.module.module().get_output("frequency").unwrap()
        })
        .collect();
    assert_eq!(a, b, "config seed flows through the factory");
}
