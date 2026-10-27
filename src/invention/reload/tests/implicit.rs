//! Reloading the original after authored writes to keys its file leaves
//! implicit: a default it omits, an indexed key (a mixer's `level.N`, a
//! melody's `degree.N`), a key it stores as a null or an array. The stored
//! config does not record such a write, so reload restores it, by a control
//! write to the value a fresh build of the file plays, without rebuilding.

use serde_json::json;

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::rpc::StructuralEdit;
use crate::ControlWriteIntent;

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn start_pumped(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    start_manual(runtime)
}

fn saved(running: &RunningInvention) -> Invention {
    let json = running.document().unwrap().to_json().unwrap();
    Invention::from_json(&json).unwrap()
}

fn assert_nothing_rebuilt(report: &ReloadReport) {
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
}

/// Oscillator `tone` into mixer `mix`, whose config is `mixer`, into the dac.
fn mixed(mixer: serde_json::Value) -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "tone", "type": "oscillator", "config": { "waveform": "sine" } },
            { "id": "mix", "type": "mixer", "config": mixer },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "tone", "from_port": "audio", "to": "mix", "to_port": "audio.1" },
            { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" }
        ]
    })
    .to_string()
}

/// A melody `tune` whose config is `melody`, into the dac.
fn melody(melody: serde_json::Value) -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "tune", "type": "melody", "config": melody },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "tune", "from_port": "frequency", "to": "dac", "to_port": "audio" }
        ]
    })
    .to_string()
}

/// An authored write to `id.key` of `original`, applied, then a reload of
/// `original`: the write is undone by one control write to `restored`, and
/// nothing is rebuilt.
fn restores(
    original: &str,
    (id, key): (&str, &str),
    written: ControlValue,
    restored: ControlValue,
) {
    let (mut running, pump) = start_pumped(original);
    assert_eq!(running.get_control(id, key).unwrap(), restored, "{key}");
    running.set_control(id, key, written).unwrap();
    pump.render(1);
    assert_ne!(running.get_control(id, key).unwrap(), restored, "{key}");

    let report = running.reload(doc(original)).expect("diff applies");
    pump.render(1);

    assert_eq!(
        report.controls_updated,
        [format!("{id}.{key}")],
        "{report:?}"
    );
    assert_nothing_rebuilt(&report);
    assert_eq!(running.get_control(id, key).unwrap(), restored, "{key}");
}

#[test]
fn reloading_the_original_restores_a_default_it_omits() {
    let omitted = BASE.replace(r#", "frequency": 440.0"#, "");
    restores(
        &omitted,
        ("osc1", "frequency"),
        number(330.0),
        number(440.0),
    );
    restores(
        BASE,
        ("osc1", "frequency_mod_depth"),
        number(5.0),
        number(0.0),
    );
    let mix = mixed(json!({ "channel_count": 2 }));
    restores(&mix, ("mix", "master"), number(0.3), number(1.0));
}

#[test]
fn reloading_the_original_restores_an_indexed_key() {
    let mix = mixed(json!({ "channel_count": 2 }));
    restores(&mix, ("mix", "level.1"), number(0.3), number(1.0));
    // Within a whole array, the index restores to the array's value.
    let mix = mixed(json!({ "channel_count": 2, "levels": [0.8, 0.6] }));
    restores(&mix, ("mix", "level.1"), number(0.3), number(0.6));
    for degrees in [json!([0, 2, 4]), json!("[0, 2, 4]")] {
        let tune = melody(json!({ "degrees": degrees, "seed": 1 }));
        restores(&tune, ("tune", "degree.1"), number(5.0), number(2.0));
    }
}

#[test]
fn reloading_the_original_restores_a_key_it_stores_as_a_null() {
    let null = BASE.replace(r#""frequency": 440.0"#, r#""frequency": null"#);
    restores(&null, ("osc1", "frequency"), number(330.0), number(440.0));
}

#[test]
fn reloading_the_original_restores_a_schedule_it_stores_as_an_array() {
    // The scheduler keeps its step: restored by a control write, as the
    // array builds it, not rebuilt.
    let original = BASE.replace(
        r#"{ "id": "dac", "type": "dac" }"#,
        r#"{ "id": "dac", "type": "dac" },
        { "id": "auto", "type": "control_scheduler", "config": { "schedule": [] } }"#,
    );
    let schedule =
        r#"[{ "at_step": 0, "module": "osc2", "control": "frequency", "value": 220.0 }]"#;
    restores(
        &original,
        ("auto", "schedule"),
        ControlValue::String(schedule.into()),
        ControlValue::String("[]".into()),
    );
}

#[test]
fn restoring_an_implicit_key_keeps_the_oscillators_phase() {
    // The reload restores osc1's omitted frequency as a twin that never
    // reloaded does with a control write at the same point, sample for
    // sample: the instance was not rebuilt.
    let omitted = BASE.replace(r#", "frequency": 440.0"#, "");
    let (mut reloaded, reloaded_pump) = start_pumped(&omitted);
    let (twin, twin_pump) = start_pumped(&omitted);
    for running in [&reloaded, &twin] {
        running
            .set_control("osc1", "frequency", number(330.0))
            .unwrap();
    }
    assert_eq!(reloaded_pump.render(7), twin_pump.render(7));

    let report = reloaded.reload(doc(&omitted)).expect("diff applies");
    assert_eq!(report.controls_updated, ["osc1.frequency"]);
    twin.snapshot()
        .set_control_with_intent(
            "osc1",
            "frequency",
            number(440.0),
            ControlWriteIntent::Perform,
        )
        .unwrap();

    assert_eq!(reloaded_pump.render(20), twin_pump.render(20));
}

#[test]
fn restoring_an_indexed_level_keeps_the_mixer_and_its_tone() {
    let mix = mixed(json!({ "channel_count": 2, "levels": [0.8, 0.6] }));
    let (mut reloaded, reloaded_pump) = start_pumped(&mix);
    let (twin, twin_pump) = start_pumped(&mix);
    for running in [&reloaded, &twin] {
        running.set_control("mix", "level.1", number(0.3)).unwrap();
    }
    assert_eq!(reloaded_pump.render(7), twin_pump.render(7));

    let report = reloaded.reload(doc(&mix)).expect("diff applies");
    assert_eq!(report.controls_updated, ["mix.level.1"]);
    twin.snapshot()
        .set_control_with_intent("mix", "level.1", number(0.6), ControlWriteIntent::Perform)
        .unwrap();

    assert_eq!(reloaded_pump.render(20), twin_pump.render(20));
}

#[test]
fn reloading_the_document_saved_after_implicit_writes_rebuilds_nothing() {
    let mix = mixed(json!({ "channel_count": 2, "levels": [0.8, 0.6] }));
    let (mut running, pump) = start_pumped(&mix);
    running.set_control("mix", "level.1", number(0.3)).unwrap();
    running.set_control("mix", "master", number(0.5)).unwrap();
    running
        .set_control("tone", "frequency_mod_depth", number(5.0))
        .unwrap();
    pump.render(1);
    let saved = saved(&running);

    let report = running.reload(saved).expect("diff applies");
    pump.render(1);

    assert_nothing_rebuilt(&report);
    for (id, key, value) in [
        ("mix", "level.1", 0.3),
        ("mix", "master", 0.5),
        ("tone", "frequency_mod_depth", 5.0),
    ] {
        assert_eq!(
            running.get_control(id, key).unwrap(),
            number(value),
            "{key}"
        );
    }
}

#[test]
fn a_performed_write_to_an_implicit_key_survives_reloading_the_original() {
    let (mut running, pump) = start_pumped(BASE);
    running
        .snapshot()
        .set_control_with_intent(
            "osc1",
            "frequency_mod_depth",
            number(5.0),
            ControlWriteIntent::Perform,
        )
        .unwrap();
    pump.render(1);

    let report = running.reload(doc(BASE)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("osc1", "frequency_mod_depth").unwrap(),
        number(5.0)
    );
}

#[test]
fn reloading_the_original_restores_an_implicit_key_an_apply_edits_batch_wrote() {
    let (mut running, pump) = start_pumped(BASE);
    running
        .apply_edits(&[StructuralEdit::SetControl {
            module_id: "osc1".into(),
            key: "frequency_mod_depth".into(),
            value: number(5.0),
        }])
        .expect("the batch commits");
    pump.render(1);

    let report = running.reload(doc(BASE)).expect("diff applies");
    pump.render(1);

    assert_eq!(report.controls_updated, ["osc1.frequency_mod_depth"]);
    assert_nothing_rebuilt(&report);
    assert_eq!(
        running.get_control("osc1", "frequency_mod_depth").unwrap(),
        number(0.0)
    );
}

#[test]
fn a_restored_key_is_restored_once() {
    // The reload adopts the original: its configs are the document's again,
    // so reloading it once more writes nothing.
    let (mut running, pump) = start_pumped(BASE);
    running
        .set_control("osc1", "frequency_mod_depth", number(5.0))
        .unwrap();
    pump.render(1);
    running.reload(doc(BASE)).expect("diff applies");

    let report = running.reload(doc(BASE)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
}

/// One development `pair` of two oscillators whose `frequency` both take
/// its exposed `pitch`, at different initial values.
const PAIR: &str = r#"{
    "version": "1.0.0",
    "developments": [{ "name": "pair", "definition": {
        "version": "1.0.0",
        "modules": [
            { "id": "a", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "b", "type": "oscillator", "config": { "frequency": 330.0 } }
        ],
        "connections": [],
        "outputs": [{ "name": "out", "from": "a", "from_port": "audio" }],
        "controls": [
            { "name": "pitch", "module": "a", "control": "frequency" },
            { "name": "pitch", "module": "b", "control": "frequency" }
        ]
    } }],
    "modules": [
        { "id": "pair", "type": "pair" },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [ { "from": "pair", "from_port": "out", "to": "dac", "to_port": "audio" } ]
}"#;

#[test]
fn a_developments_implicit_key_is_not_restored_by_one_write() {
    // One write cannot put back two inner values, and a development's
    // config applies after a validation build reads it: reloading keeps
    // what was written rather than restoring a value the file never plays.
    let (mut running, pump) = start_pumped(PAIR);
    running.set_control("pair", "pitch", number(110.0)).unwrap();
    pump.render(1);

    let report = running.reload(doc(PAIR)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_nothing_rebuilt(&report);
    assert_eq!(running.get_control("pair", "pitch").unwrap(), number(110.0));
}

#[test]
fn a_fresh_value_no_write_reaches_is_not_restored() {
    // An empty scale builds a count of zero, below the one its writes
    // start at: no write restores it, so none is made.
    let tune = melody(json!({ "degrees": [], "seed": 1 }));
    let (mut running, pump) = start_pumped(&tune);
    assert_eq!(
        running.get_control("tune", "degree_count").unwrap(),
        number(0.0)
    );
    running
        .set_control("tune", "degree_count", number(2.0))
        .unwrap();
    pump.render(1);

    let report = running.reload(doc(&tune)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_nothing_rebuilt(&report);
}
