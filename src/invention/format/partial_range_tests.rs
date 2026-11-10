//! A development control with a partial range fails the load with an error
//! naming the development, the control and the missing bound; two-sided
//! ranges, duplicate keys and the array form read as they always did.

use serde_json::json;

use super::*;
use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::reload::DevelopmentDefinitions;

fn development(range: serde_json::Value) -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "modules": [{ "id": "f", "type": "filter" }],
        "connections": [],
        "controls": [
            { "name": "resonance", "module": "f", "control": "resonance" },
            { "name": "cutoff", "module": "f", "control": "cutoff", "range": range }
        ]
    })
}

/// A document declaring `voice` inline, its keys in sorted order (the
/// definition before the name), as one built from a JSON map writes them.
fn sorted_document(range: serde_json::Value) -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": development(range) }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    }))
    .unwrap()
}

/// The same, as text with the name before the definition.
fn written_document(range: serde_json::Value) -> Invention {
    Invention::from_json(&format!(
        r#"{{ "developments": [{{ "name": "voice", "definition": {} }}],
             "modules": [{{ "id": "v", "type": "voice" }}], "connections": [] }}"#,
        development(range)
    ))
    .unwrap()
}

fn load_error(document: &Invention) -> String {
    DevelopmentDefinitions::resolve(document)
        .expect_err("a partial range must not load")
        .to_string()
}

fn names(message: &str, missing: &str) {
    let expected = format!(
        "Development 'voice': control 'cutoff': range is missing {missing}: \
         a range needs both `min` and `max`"
    );
    assert_eq!(message, expected);
}

#[test]
fn a_range_without_min_names_the_development_control_and_bound() {
    let range = json!({ "max": 10 });
    names(&load_error(&sorted_document(range.clone())), "`min`");
    names(&load_error(&written_document(range)), "`min`");
}

#[test]
fn a_range_without_max_names_the_development_control_and_bound() {
    let range = json!({ "min": 0 });
    names(&load_error(&sorted_document(range.clone())), "`max`");
    names(&load_error(&written_document(range)), "`max`");
}

#[test]
fn a_range_with_neither_bound_names_both() {
    names(&load_error(&sorted_document(json!({}))), "`min` and `max`");
}

#[test]
fn a_null_bound_is_missing() {
    let range = json!({ "min": null, "max": 10 });
    names(&load_error(&sorted_document(range)), "`min`");
}

#[test]
fn a_file_development_with_a_partial_range_names_the_development_and_bound() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("voice.json"),
        development(json!({ "max": 10 })).to_string(),
    )
    .unwrap();
    let mut parent: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "path": "voice.json" }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    }))
    .unwrap();
    parent.source_path = Some(dir.path().join("parent.json"));
    names(&load_error(&parent), "`min`");
}

#[test]
fn a_partial_range_in_a_nested_development_names_the_nested_one() {
    let nested = json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": development(json!({ "min": 0 })) }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    });
    let document: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "outer", "definition": nested }],
        "modules": [{ "id": "o", "type": "outer" }],
        "connections": []
    }))
    .unwrap();
    names(&load_error(&document), "`max`");
}

#[test]
fn a_complete_range_loads() {
    let document = sorted_document(json!({ "min": 0, "max": 10 }));
    DevelopmentDefinitions::resolve(&document).unwrap();
}

#[test]
fn a_duplicate_bound_in_a_range_is_refused_as_ever() {
    let control = r#"{ "name": "cutoff", "module": "f", "control": "cutoff",
                       "range": { "min": 0, "min": 1, "max": 10 } }"#;
    let error = serde_json::from_str::<DevelopmentControl>(control)
        .expect_err("a duplicate bound must not parse")
        .to_string();
    assert!(error.contains("duplicate field `min`"), "{error}");
}

#[test]
fn a_duplicate_field_in_an_inline_definition_is_refused_in_either_order() {
    let definition = r#"{
        "modules": [{ "id": "f", "type": "filter" }],
        "connections": [],
        "connections": [{ "from": "f", "from_port": "out", "to": "f", "to_port": "in" }]
    }"#;
    for spec in [
        format!(r#"{{ "name": "voice", "definition": {definition} }}"#),
        format!(r#"{{ "definition": {definition}, "name": "voice" }}"#),
    ] {
        let document = format!(
            r#"{{ "developments": [{spec}],
                 "modules": [{{ "id": "v", "type": "voice" }}], "connections": [] }}"#
        );
        let error = Invention::from_json(&document)
            .expect_err("a duplicate field must not parse")
            .to_string();
        assert!(error.contains("duplicate field `connections`"), "{error}");
    }
}

#[test]
fn a_range_of_the_wrong_shape_expects_a_control_range() {
    let control = r#"{ "name": "a", "module": "f", "control": "a", "range": 5 }"#;
    let error = serde_json::from_str::<DevelopmentControl>(control)
        .expect_err("a number is not a range")
        .to_string();
    assert!(error.contains("expected struct ControlRange"), "{error}");
}

#[test]
fn two_sided_ranges_read_as_they_always_did() {
    for text in [
        r#"{ "min": 0, "max": 10 }"#,
        r#"{ "max": 0.5, "min": -0.25 }"#,
        r#"{ "min": 20, "max": 20000.5 }"#,
        r#"{ "min": 0.1, "max": 9007199254740993 }"#,
    ] {
        let direct: ControlRange = serde_json::from_str(text).unwrap();
        let alias = format!(
            r#"{{ "name": "cutoff", "module": "f", "control": "cutoff", "range": {text} }}"#
        );
        let control: DevelopmentControl = serde_json::from_str(&alias).unwrap();
        assert_eq!(control.range, Some(direct), "{text}");
        assert_eq!(control.range_missing, None, "{text}");
        let saved: DevelopmentControl =
            serde_json::from_str(&serde_json::to_string(&control).unwrap()).unwrap();
        assert_eq!(saved, control, "{text}");
    }
    let bare: DevelopmentControl =
        serde_json::from_str(r#"{ "name": "a", "module": "f", "control": "a" }"#).unwrap();
    assert_eq!(bare.range, None);
}

#[test]
fn the_array_form_still_parses() {
    let control: DevelopmentControl =
        serde_json::from_str(r#"["cutoff", "f", "cutoff", { "min": 0, "max": 1 }]"#).unwrap();
    assert_eq!(control.range, Some(ControlRange { min: 0.0, max: 1.0 }));
    let spec: DevelopmentSpec =
        serde_json::from_str(r#"["voice", "voice.json", null, null]"#).unwrap();
    assert_eq!(spec.name, "voice");
    assert_eq!(spec.path.as_deref(), Some("voice.json"));
}

/// A document with a dac and its own `cutoff` control, whose range is
/// `range`, as one written to be built as a development holds.
fn root_document(range: serde_json::Value) -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [{ "id": "dac", "type": "dac" }],
        "connections": [],
        "controls": [{ "name": "cutoff", "module": "dac", "control": "level", "range": range }]
    }))
    .unwrap()
}

const ROOT_REFUSAL: &str =
    "control 'cutoff': range is missing `min`: a range needs both `min` and `max`";

#[test]
fn a_root_document_with_a_partial_range_does_not_build() {
    let error = InventionBuilder::new(SAMPLE_RATE)
        .build(root_document(json!({ "max": 10 })))
        .err()
        .expect("a partial range must not build")
        .to_string();
    assert_eq!(error, ROOT_REFUSAL);
}

#[test]
fn a_root_document_names_its_file_when_it_has_one() {
    let mut document = root_document(json!({ "max": 10 }));
    document.source_path = Some("/songs/voice.json".into());
    let error = InventionBuilder::new(SAMPLE_RATE)
        .build(document)
        .err()
        .expect("a partial range must not build")
        .to_string();
    assert_eq!(error, format!("/songs/voice.json: {ROOT_REFUSAL}"));
}

#[test]
fn a_reload_to_a_root_document_with_a_partial_range_is_refused() {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(root_document(json!({ "min": 0, "max": 10 })))
        .unwrap();
    let (mut running, _pump) = start_manual(runtime);
    let error = running
        .reload(root_document(json!({ "max": 10 })))
        .expect_err("a partial range must not reload")
        .to_string();
    assert!(error.contains(ROOT_REFUSAL), "{error}");
    // A complete range reloads.
    running
        .reload(root_document(json!({ "min": 0, "max": 5 })))
        .unwrap();
}
