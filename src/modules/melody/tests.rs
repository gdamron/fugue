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

    // Past the scale [0, 2, 3, 5, 7, 9, 10], degrees repeat it from the start.
    assert_eq!(melody.get_control("degree.7").unwrap(), 0.0);
    assert_eq!(melody.get_control("degree.8").unwrap(), 2.0);

    // With no weights of their own, they repeat the scale's (1.0 here).
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
fn shrinking_hides_degrees_and_growing_shows_them_as_they_were() {
    let mut melody = make_melody();
    melody.set_control("degree.6", 11.0).unwrap();
    melody.set_control("note_weight.5", 4.0).unwrap();

    melody.set_control("degree_count", 3.0).unwrap();
    assert!(melody.get_control("degree.6").is_err());
    melody.set_control("degree_count", 7.0).unwrap();

    let degrees: Vec<f32> = (0..7)
        .map(|i| melody.get_control(&format!("degree.{i}")).unwrap())
        .collect();
    assert_eq!(degrees, [0.0, 2.0, 3.0, 5.0, 7.0, 9.0, 11.0]);
    assert_eq!(melody.get_control("note_weight.5").unwrap(), 4.0);
}

#[test]
fn growing_past_the_scale_repeats_its_degrees_and_weights() {
    let controls = MelodyControls::new(60, vec![0, 4, 7]);
    controls.set_note_weights(vec![3.0, 1.0, 2.0]);
    controls.set_degree_count(8);
    assert_eq!(controls.allowed_degrees(), [0, 4, 7, 0, 4, 7, 0, 4]);
    assert_eq!(
        controls.note_weights(),
        [3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]
    );

    // A weight configured past the scale is that position's own.
    controls.set_note_weights(vec![3.0, 1.0, 2.0, 9.0]);
    assert_eq!(controls.note_weights()[3..5], [9.0, 1.0]);
}

#[test]
fn the_table_is_the_same_whatever_order_its_sources_are_set_in() {
    // What a saved document records (the count and single-position writes)
    // rebuilds the table the live writes made, in any order.
    let live = MelodyControls::new(60, vec![0, 2, 4, 5, 7, 9, 11]);
    live.set_degree(6, 1).unwrap();
    live.set_degree_count(3);
    live.set_degree(1, 3).unwrap();
    live.set_degree_count(10);
    live.set_note_weight(8, 5.0).unwrap();
    live.set_degree_count(9);

    let rebuilt = MelodyControls::new(60, vec![0, 2, 4, 5, 7, 9, 11]);
    rebuilt.set_degree_count(9);
    rebuilt.restore_written([(1, 3), (6, 1)], [(8, 5.0)]);
    assert_eq!(rebuilt.allowed_degrees(), live.allowed_degrees());
    assert_eq!(rebuilt.note_weights(), live.note_weights());
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
            "degrees": [0, 2, 4, 5, 7],
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

#[test]
fn a_degree_recorded_past_a_later_shrunk_count_is_hidden_on_rebuild() {
    // `degree.6` was written while the scale had seven degrees, then the
    // count shrank to three; the config still holds the stale index.
    let config = serde_json::json!({ "degree_count": 3, "degree.6": 11, "note_weight.6": 2.0 });
    let built = crate::ModuleRegistry::default()
        .build("melody", 48_000, &config)
        .expect("the index is hidden, not an error");
    let surface = built.control_surface.unwrap();
    assert_eq!(
        surface.get_control("degree_count").unwrap(),
        crate::ControlValue::Number(3.0)
    );
    assert!(surface.get_control("degree.6").is_err());

    // Growing shows it again, as it was written.
    surface
        .set_control("degree_count", crate::ControlValue::Number(7.0))
        .unwrap();
    assert_eq!(
        surface.get_control("degree.6").unwrap(),
        crate::ControlValue::Number(11.0)
    );
}

#[test]
fn an_empty_scale_grows_with_degree_zero() {
    let controls = MelodyControls::new(60, vec![]);
    assert_eq!(controls.degree_count(), 0);
    controls.set_degree_count(3);
    assert_eq!(controls.allowed_degrees(), [0, 0, 0]);
    controls.set_degree(1, 7).unwrap();
    assert_eq!(controls.allowed_degrees(), [0, 7, 0]);
}

#[test]
fn single_position_and_count_writes_do_not_allocate() {
    // A control_scheduler ramp may write a degree or weight every sample,
    // from the audio thread.
    let controls = MelodyControls::new(60, vec![0, 2, 4, 5, 7]);
    controls.set_note_weights(vec![1.0, 2.0]);
    controls.set_degree_count(controls::MAX_DEGREES);
    controls.set_degree_count(5);
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        for step in 0..64 {
            controls.set_note_weight(0, step as f32 / 64.0).unwrap();
            controls.set_degree(1, step % 12).unwrap();
            controls.set_degree_count(3 + step as usize % 9);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn indexed_config_values_are_taken_as_the_setters_take_them() {
    let registry = crate::ModuleRegistry::default();
    let built = registry
        .build(
            "melody",
            48_000,
            &serde_json::json!({ "degree.0": "7", "note_weight.1": "0" }),
        )
        .unwrap();
    let surface = built.control_surface.unwrap();
    assert_eq!(
        surface.get_control("degree.0").unwrap(),
        crate::ControlValue::Number(7.0)
    );
    assert_eq!(
        surface.get_control("note_weight.1").unwrap(),
        crate::ControlValue::Number(0.0)
    );

    let near = registry
        .build(
            "melody",
            48_000,
            &serde_json::json!({ "degree.0": 1.99999999 }),
        )
        .unwrap();
    assert_eq!(
        near.control_surface
            .unwrap()
            .get_control("degree.0")
            .unwrap(),
        crate::ControlValue::Number(2.0)
    );

    for config in [
        serde_json::json!({ "degree.0": "high" }),
        serde_json::json!({ "note_weight.1": true }),
        serde_json::json!({ "degree.x": 3 }),
    ] {
        assert!(
            registry.build("melody", 48_000, &config).is_err(),
            "{config}"
        );
    }
}

#[test]
fn an_unchanged_count_is_not_an_edit() {
    let controls = MelodyControls::new(60, vec![0, 2, 4, 5, 7]);
    let version = controls.table_version();
    controls.set_degree_count(5);
    assert_eq!(controls.table_version(), version);
    controls.set_degree_count(4);
    assert_ne!(controls.table_version(), version);
}

/// The controls a melody built from `config` starts with, or the refusal.
fn built_controls(config: serde_json::Value) -> Result<MelodyControls, String> {
    use crate::factory::ModuleFactory;
    let built = MelodyFactory
        .build(48_000, &config)
        .map_err(|error| error.to_string())?;
    let (_, handle) = &built.handles[0];
    Ok(handle.downcast_ref::<MelodyControls>().unwrap().clone())
}

#[test]
fn a_whole_float_root_note_and_seed_read_as_written() {
    let controls = built_controls(serde_json::json!({ "root_note": 72.0, "seed": 99.0 })).unwrap();
    assert_eq!(controls.root_note(), 72);
    assert_eq!(controls.seed(), Some(99));
}

#[test]
fn a_fractional_or_out_of_range_root_note_is_refused() {
    let error = built_controls(serde_json::json!({ "root_note": 72.5 })).err();
    assert_eq!(
        error.as_deref(),
        Some("melody config 'root_note' expects a whole number from 0 to 255, got 72.5")
    );
    // Was read `as u8`, so 256 played as 0.
    let error = built_controls(serde_json::json!({ "root_note": 256 })).err();
    assert_eq!(
        error.as_deref(),
        Some("melody config 'root_note' expects a whole number from 0 to 255, got 256")
    );
}

#[test]
fn degrees_and_note_weights_refuse_a_non_number_rather_than_drop_it() {
    let controls =
        built_controls(serde_json::json!({ "degrees": [0, 4.0, 7], "note_weights": [1, 0.5] }))
            .unwrap();
    assert_eq!(controls.allowed_degrees(), [0, 4, 7]);
    assert_eq!(controls.note_weights(), [1.0, 0.5, 1.0]);
    for (config, refusal) in [
        (
            serde_json::json!({ "degrees": [0, "4", 7] }),
            "melody config 'degrees[1]' expects a whole number",
        ),
        (
            serde_json::json!({ "degrees": [0, 4.5] }),
            "melody config 'degrees[1]' expects a whole number",
        ),
        (
            serde_json::json!({ "note_weights": [1, null] }),
            "melody config 'note_weights[1]' expects a finite number, got null",
        ),
        (
            serde_json::json!({ "note_weights": [1e39] }),
            "melody config 'note_weights[0]' expects a finite number, got 1e39",
        ),
        (
            serde_json::json!({ "note_weights": 2 }),
            "melody config 'note_weights' expects an array of numbers, got 2",
        ),
    ] {
        let error = built_controls(config.clone()).err().unwrap_or_default();
        assert!(error.starts_with(refusal), "{config}: {error}");
    }
}
