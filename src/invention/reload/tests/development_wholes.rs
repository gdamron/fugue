//! A development exposing a whole value beside the keys it replaces: its
//! saved config forgets them as the inner module's would, as far as one
//! value per exposed key can hold.

use super::*;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn start_pumped(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    start_manual(runtime)
}

/// The retained document, as saving writes it.
fn saved(running: &RunningInvention) -> String {
    running.document().unwrap().to_json().unwrap()
}

fn read(running: &RunningInvention, id: &str, key: &str) -> ControlValue {
    running.get_control(id, key).unwrap()
}

/// A document of one development `line`, defined by `modules` and
/// `controls`, sounding module `a`'s `port`, its instance configured by
/// `config`.
fn line(modules: &str, controls: &str, port: &str, config: &str) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{ "name": "line", "definition": {{
            "version": "1.0.0",
            "modules": [{modules}],
            "connections": [],
            "outputs": [{{ "name": "out", "from": "a", "from_port": "{port}" }}],
            "controls": [{controls}]
        }} }}],
        "modules": [
            {{ "id": "line", "type": "line", "config": {config} }},
            {{ "id": "dac", "type": "dac" }}
        ],
        "connections": [
            {{ "from": "line", "from_port": "out", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

const TUNES: &str = r#"{ "id": "a", "type": "melody", "config": { "seed": 1 } },
    { "id": "b", "type": "melody", "config": { "seed": 2 } }"#;

#[test]
fn a_developments_whole_scale_forgets_the_count_it_replaces_from_the_saved_document() {
    let json = line(
        r#"{ "id": "a", "type": "melody", "config": { "seed": 1 } }"#,
        r#"{ "name": "count", "module": "a", "control": "degree_count" },
           { "name": "scale", "module": "a", "control": "degrees" }"#,
        "frequency",
        r#"{ "count": 5 }"#,
    );
    let (running, pump) = start_pumped(&json);
    running
        .set_control("line", "scale", ControlValue::String("[0, 7]".into()))
        .unwrap();
    pump.render(1);
    assert_eq!(read(&running, "line", "count"), number(2.0));
    // A cold build of the saved document plays what the live one does: the
    // scale applies before any count, so a count it replaced must be gone.
    let (cold, pump) = start_pumped(&saved(&running));
    pump.render(1);
    assert_eq!(
        read(&cold, "line", "count"),
        read(&running, "line", "count")
    );
    assert_eq!(
        read(&cold, "line", "scale"),
        read(&running, "line", "scale")
    );
}

#[test]
fn a_count_fanning_out_beyond_the_scales_melody_keeps_its_value_in_the_saved_document() {
    let json = line(
        TUNES,
        r#"{ "name": "count", "module": "a", "control": "degree_count" },
           { "name": "count", "module": "b", "control": "degree_count" },
           { "name": "scale", "module": "b", "control": "degrees" }"#,
        "frequency",
        r#"{ "count": 5 }"#,
    );
    let (running, pump) = start_pumped(&json);
    running
        .set_control("line", "scale", ControlValue::String("[0, 7]".into()))
        .unwrap();
    pump.render(1);
    // The scale splits the count's two melodies apart, which one value per
    // exposed key cannot hold: the document keeps the count, as the melody
    // it leaves untouched still plays it.
    let document = doc(&saved(&running));
    let config = &document.modules[0].config;
    assert_eq!(config["count"], serde_json::json!(5), "{config}");
}

#[test]
fn a_development_of_two_voices_of_one_type_builds_with_both_keys_configured() {
    // Its voices share their type's table: the debug check of rewrites
    // must tell them apart, and a key's own targets are not rewrites.
    let json = line(
        r#"{ "id": "a", "type": "oscillator", "config": {} },
           { "id": "b", "type": "oscillator", "config": {} }"#,
        r#"{ "name": "low", "module": "a", "control": "frequency" },
           { "name": "high", "module": "b", "control": "frequency" },
           { "name": "both", "module": "a", "control": "frequency" },
           { "name": "both", "module": "b", "control": "frequency" }"#,
        "audio",
        r#"{ "low": 220, "high": 330, "both": 110 }"#,
    );
    let (running, pump) = start_pumped(&json);
    pump.render(1);
    assert_eq!(read(&running, "line", "low"), number(220.0));
    assert_eq!(read(&running, "line", "high"), number(330.0));
}

#[test]
fn a_key_a_scale_replaces_only_part_of_keeps_its_value_in_the_saved_document() {
    // `shape` reaches the count, which the scale replaces, and the root,
    // which it does not.
    let json = line(
        r#"{ "id": "a", "type": "melody", "config": { "seed": 1 } }"#,
        r#"{ "name": "shape", "module": "a", "control": "degree_count" },
           { "name": "shape", "module": "a", "control": "root_note" },
           { "name": "scale", "module": "a", "control": "degrees" }"#,
        "frequency",
        r#"{ "shape": 5 }"#,
    );
    let (running, pump) = start_pumped(&json);
    running
        .set_control("line", "scale", ControlValue::String("[0, 7]".into()))
        .unwrap();
    pump.render(1);
    let document = doc(&saved(&running));
    let config = &document.modules[0].config;
    assert_eq!(config["shape"], serde_json::json!(5), "{config}");
}
