use super::*;
use crate::{ControlValue, ModuleBuildResult};

fn make_melody() -> MelodyGenerator {
    MelodyGenerator::new(60, vec![0, 2, 3, 5, 7, 9, 10])
}

/// Writes whole-list control `key` (`degrees`, `note_weights`) as its JSON
/// text, as the drain would.
fn write_list(melody: &mut MelodyGenerator, key: &str, text: &str) {
    let index = TABLE.resolve(key).unwrap();
    let codec = TABLE.decl(index).unwrap().0.codec.unwrap();
    let (payload, _) = (codec.prepare)(&text.into()).unwrap();
    let applied = melody.apply_payload(index, payload);
    assert!(applied.is_ok(), "{key} {text}");
}

fn keys(controls: &[ControlMeta]) -> Vec<&str> {
    controls.iter().map(|meta| meta.key.as_str()).collect()
}

#[test]
fn the_positions_in_play_are_listed() {
    let mut melody = make_melody();
    let controls = Module::controls(&melody);
    // root_note, degree_count, seed + 7 degrees + 7 weights; the whole
    // lists are payloads, which no number holds.
    assert_eq!(controls.len(), 17);
    let listed = keys(&controls);
    for key in [
        "root_note",
        "degree_count",
        "seed",
        "degree.6",
        "note_weight.6",
    ] {
        assert!(listed.contains(&key), "{key}");
    }
    assert!(!listed.contains(&"degree.7"));

    melody.set_control("degree_count", 9.0).unwrap();
    assert_eq!(Module::controls(&melody).len(), 21);
}

#[test]
fn positions_read_and_write_as_numbers() {
    let mut melody = make_melody();
    assert_eq!(melody.get_control("degree_count").unwrap(), 7.0);
    assert_eq!(melody.get_control("degree.3").unwrap(), 5.0);
    assert_eq!(melody.get_control("note_weight.0").unwrap(), 1.0);

    melody.set_control("degree.2", 5.0).unwrap();
    melody.set_control("note_weight.1", 3.5).unwrap();
    melody.set_control("degree.0", -3.0).unwrap();
    assert_eq!(melody.get_control("degree.2").unwrap(), 5.0);
    assert_eq!(melody.get_control("note_weight.1").unwrap(), 3.5);
    assert_eq!(melody.get_control("degree.0").unwrap(), -3.0);
    assert!(melody.get_control("unknown").is_err());
    assert!(melody.get_control("degree.128").is_err());
}

#[test]
fn whole_numbers_are_truncated_and_clamped_as_written() {
    let mut melody = make_melody();
    for (key, written, held) in [
        ("degree.1", 4.7, 4.0),
        ("degree.1", -2.7, -2.0),
        ("degree.1", 300.0, 127.0),
        ("degree_count", 3.9, 3.0),
        ("degree_count", 0.0, 1.0),
        ("degree_count", 500.0, 128.0),
        ("root_note", 61.5, 61.0),
        ("root_note", 200.0, 127.0),
        ("note_weight.0", 12.0, 10.0),
    ] {
        melody.set_control(key, written).unwrap();
        assert_eq!(melody.get_control(key).unwrap(), held, "{key} {written}");
    }
}

#[test]
fn growing_the_count_repeats_the_scale_and_shows_hidden_positions() {
    let mut melody = make_melody();
    melody.set_control("degree.6", 11.0).unwrap();
    melody.set_control("degree_count", 3.0).unwrap();
    // A position past the count is kept, and written, while hidden.
    melody.set_control("note_weight.5", 4.0).unwrap();
    melody.set_control("degree_count", 9.0).unwrap();
    let degrees: Vec<f32> = (0..9)
        .map(|i| melody.get_control(&format!("degree.{i}")).unwrap())
        .collect();
    assert_eq!(degrees, [0.0, 2.0, 3.0, 5.0, 7.0, 9.0, 11.0, 0.0, 2.0]);
    assert_eq!(melody.get_control("note_weight.5").unwrap(), 4.0);
    assert_eq!(melody.get_control("note_weight.8").unwrap(), 1.0);
}

#[test]
fn a_whole_scale_and_whole_weights_republish_every_position() {
    let mut melody = make_melody();
    melody.set_control("degree.1", 9.0).unwrap();
    melody.set_control("note_weight.4", 2.0).unwrap();
    write_list(&mut melody, "degrees", "[0, 4, 7]");
    assert_eq!(melody.get_control("degree_count").unwrap(), 3.0);
    assert_eq!(melody.get_control("degree.1").unwrap(), 4.0);
    // Written weights are kept; positions past the scale repeat it.
    assert_eq!(melody.get_control("note_weight.4").unwrap(), 2.0);
    assert_eq!(melody.get_control("degree.4").unwrap(), 4.0);

    write_list(&mut melody, "note_weights", "[3, 1]");
    assert_eq!(melody.get_control("note_weight.4").unwrap(), 1.0);
    assert_eq!(melody.get_control("note_weight.3").unwrap(), 3.0);
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
    let mut melody = make_melody();
    melody.set_control("seed", seed as f32).unwrap();
    melody
}

#[test]
fn the_same_seed_produces_identical_melodies() {
    let a = notes(&mut seeded_melody(42), 32);
    let b = notes(&mut seeded_melody(42), 32);
    assert_eq!(a, b, "same seed must reproduce the same note stream");
    let c = notes(&mut seeded_melody(43), 32);
    assert_ne!(a, c, "different seeds should diverge");
}

#[test]
fn reseeding_restarts_the_stream() {
    let mut melody = seeded_melody(7);
    let first = notes(&mut melody, 8);
    // Setting the seed it holds again restarts the stream from the top.
    melody.set_control("seed", 7.0).unwrap();
    assert_eq!(notes(&mut melody, 8), first);
    assert_eq!(melody.get_control("seed").unwrap(), 7.0);
}

#[test]
fn degree_root_and_weight_changes_reach_the_notes() {
    let mut melody = seeded_melody(9);
    write_list(&mut melody, "degrees", "[0, 4]");
    write_list(&mut melody, "note_weights", "[0, 1]");
    assert_eq!(pulse_note(&mut melody), Note::new(64).frequency());

    melody.set_control("root_note", 62.0).unwrap();
    assert_eq!(pulse_note(&mut melody), Note::new(66).frequency());

    melody.set_control("note_weight.0", 1.0).unwrap();
    melody.set_control("note_weight.1", 0.0).unwrap();
    assert_eq!(pulse_note(&mut melody), Note::new(62).frequency());
}

#[test]
fn no_degrees_yields_middle_c() {
    let mut melody = MelodyGenerator::new(67, vec![]);
    assert_eq!(melody.next_note().midi_note, 60);
}

/// Every write the audio thread applies, and the notes they shape, allocate
/// and free nothing: a scheduler may ramp any of them every sample.
#[test]
fn writes_and_the_notes_they_shape_do_not_allocate() {
    let mut melody = seeded_melody(3);
    let index = |key: &str| TABLE.resolve(key).unwrap();
    let scale = index("degrees");
    let codec = TABLE.decl(scale).unwrap().0.codec.unwrap();
    let mut payloads: Vec<_> = (0..8)
        .map(|n| (codec.prepare)(&format!("[0, 2, {n}]").into()).unwrap().0)
        .collect();
    let mut retired = Vec::with_capacity(payloads.len());
    let keys = [
        "degree_count",
        "degree.4",
        "note_weight.4",
        "root_note",
        "seed",
    ];
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        let _audio = crate::audio_thread::AudioThreadScope::enter();
        for step in 0..64 {
            for key in keys {
                let value = RtValue::F32(step as f32 / 4.0);
                crate::control_request::apply_declared(&mut melody, index(key), value).unwrap();
            }
            if let Some(payload) = payloads.pop() {
                retired.push(melody.apply_payload(scale, payload).ok().unwrap());
            }
            pulse_note(&mut melody);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
}

fn built(config: serde_json::Value) -> Result<ModuleBuildResult, String> {
    MelodyFactory
        .build(48_000, &config)
        .map_err(|error| error.to_string())
}

fn read(built: &ModuleBuildResult, key: &str) -> ControlValue {
    built
        .control_surface
        .as_ref()
        .unwrap()
        .get_control(key)
        .unwrap()
}

#[test]
fn a_config_seed_flows_through_the_factory() {
    let config = serde_json::json!({ "root_note": 60, "degrees": [0, 2, 4, 5, 7], "seed": 99 });
    let run = |built: &mut ModuleBuildResult| -> Vec<f32> {
        let melody = built.module.module_mut();
        (0..16)
            .map(|_| {
                for gate in [1.0, 0.0] {
                    melody.set_input("gate", gate).unwrap();
                    melody.process(1);
                }
                melody.get_output("frequency").unwrap()
            })
            .collect()
    };
    let first = run(&mut built(config.clone()).unwrap());
    assert_eq!(first, run(&mut built(config).unwrap()));
}

#[test]
fn a_degree_recorded_past_a_later_shrunk_count_is_hidden_on_rebuild() {
    // `degree.6` was written while the scale had seven degrees, then the
    // count shrank to three; the config still holds the index.
    let config = serde_json::json!({ "degree_count": 3, "degree.6": 11, "note_weight.6": 2.0 });
    let mut built = built(config).expect("the index is hidden, not an error");
    assert_eq!(read(&built, "degree_count"), ControlValue::Number(3.0));
    let listed = built.control_surface.as_ref().unwrap().controls();
    assert!(!keys(&listed).contains(&"degree.6"));

    // Growing shows it again, as it was written.
    let melody = built.module.module_mut();
    melody.set_control("degree_count", 7.0).unwrap();
    assert_eq!(melody.get_control("degree.6").unwrap(), 11.0);
    assert_eq!(melody.get_control("note_weight.6").unwrap(), 2.0);
}

#[test]
fn indexed_config_values_are_taken_as_writes_take_them() {
    let built_ok = built(serde_json::json!({ "degree.0": "7", "note_weight.1": "0" })).unwrap();
    assert_eq!(read(&built_ok, "degree.0"), ControlValue::Number(7.0));
    assert_eq!(read(&built_ok, "note_weight.1"), ControlValue::Number(0.0));

    let near = built(serde_json::json!({ "degree.0": 1.99999999 })).unwrap();
    assert_eq!(read(&near, "degree.0"), ControlValue::Number(2.0));

    for config in [
        serde_json::json!({ "degree.0": "high" }),
        serde_json::json!({ "note_weight.1": true }),
        serde_json::json!({ "degree.x": 3 }),
    ] {
        assert!(built(config.clone()).is_err(), "{config}");
    }
}

#[test]
fn a_whole_float_root_note_and_seed_read_as_written() {
    let built = built(serde_json::json!({ "root_note": 72.0, "seed": 99.0 })).unwrap();
    assert_eq!(read(&built, "root_note"), ControlValue::Number(72.0));
    assert_eq!(read(&built, "seed"), ControlValue::Number(99.0));
}

#[test]
fn a_fractional_or_out_of_range_root_note_is_refused() {
    assert_eq!(
        built(serde_json::json!({ "root_note": 72.5 }))
            .err()
            .as_deref(),
        Some("melody config 'root_note' expects a whole number from 0 to 255, got 72.5")
    );
    // Was read `as u8`, so 256 played as 0.
    assert_eq!(
        built(serde_json::json!({ "root_note": 256 }))
            .err()
            .as_deref(),
        Some("melody config 'root_note' expects a whole number from 0 to 255, got 256")
    );
}

#[test]
fn whole_lists_are_read_as_arrays_or_as_the_text_a_write_records() {
    let config = serde_json::json!({ "degrees": [0, 4.0, 7], "note_weights": [1, 0.5] });
    let built_ok = built(config).unwrap();
    assert_eq!(
        read(&built_ok, "degrees"),
        ControlValue::String("[0,4,7]".into())
    );
    assert_eq!(
        read(&built_ok, "note_weights"),
        ControlValue::String("[1.0,0.5]".into())
    );
    assert_eq!(read(&built_ok, "note_weight.2"), ControlValue::Number(1.0));

    let text = built(serde_json::json!({ "degrees": "[0,4,7]", "note_weights": "[2]" })).unwrap();
    assert_eq!(read(&text, "degree.2"), ControlValue::Number(7.0));
    assert_eq!(read(&text, "note_weight.0"), ControlValue::Number(2.0));
    assert_eq!(
        read(&text, "note_weight.3"),
        ControlValue::Number(2.0),
        "repeats the scale's"
    );

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
        let error = built(config.clone()).err().unwrap_or_default();
        assert!(error.starts_with(refusal), "{config}: {error}");
    }
}

/// Before the melody runs, its surface holds a written number as the
/// melody will once it applies it.
#[test]
fn a_built_surface_holds_its_whole_numbers_truncated() {
    let built = built(serde_json::json!({})).unwrap();
    let surface = built.control_surface.as_ref().unwrap();
    for (key, written, held) in [
        ("root_note", 61.75, 61.0),
        ("degree_count", 3.5, 3.0),
        ("seed", 9.9, 9.0),
        ("degree.1", -2.5, -2.0),
        ("note_weight.1", 2.5, 2.5),
    ] {
        surface.set_control(key, written.into()).unwrap();
        assert_eq!(read(&built, key), ControlValue::Number(held), "{key}");
    }
}

/// Automation holds a whole number truncated as it writes it, so a ramp
/// starting from a write still waiting starts where the melody will be.
#[test]
fn automation_holds_its_whole_numbers_truncated() {
    let built = built(serde_json::json!({})).unwrap();
    let surface = built.control_surface.as_ref().unwrap();
    for (key, written, held) in [
        ("root_note", 61.75, 61.0),
        ("degree.0", 4.7, 4.0),
        ("degree.1", -2.5, -2.0),
        ("note_weight.1", 2.5, 2.5),
    ] {
        let automation = surface.automation(key).unwrap();
        automation.write_number(written);
        assert_eq!(automation.current(), Some(held), "{key}");
    }
}
