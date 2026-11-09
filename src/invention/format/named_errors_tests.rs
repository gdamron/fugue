//! A development control with a partial range fails with an error naming
//! the development, the control and the missing bound; two-sided ranges
//! read as they always did.

use serde_json::json;

use super::*;
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

fn inline_document(range: serde_json::Value) -> String {
    json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": development(range) }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    })
    .to_string()
}

fn message(result: Result<Invention, Box<dyn std::error::Error>>) -> String {
    result
        .expect_err("a partial range must not load")
        .to_string()
}

fn names(message: &str, bound: &str) {
    for expected in ["Development 'voice'", "control 'cutoff'", bound] {
        assert!(
            message.contains(expected),
            "{expected} missing from: {message}"
        );
    }
}

#[test]
fn an_inline_range_without_min_names_the_development_control_and_bound() {
    let error = message(Invention::from_json(&inline_document(json!({ "max": 10 }))));
    names(&error, "missing `min`:");
}

#[test]
fn an_inline_range_without_max_names_the_development_control_and_bound() {
    let error = message(Invention::from_json(&inline_document(json!({ "min": 0 }))));
    names(&error, "missing `max`:");
}

#[test]
fn a_range_with_neither_bound_names_both() {
    let error = message(Invention::from_json(&inline_document(json!({}))));
    names(&error, "missing `min` and `max`:");
}

#[test]
fn a_file_development_with_a_partial_range_names_the_development_and_bound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.json");
    std::fs::write(&path, development(json!({ "max": 10 })).to_string()).unwrap();
    let mut parent = Invention::from_json(
        &json!({
            "version": "1.0.0",
            "developments": [{ "name": "voice", "path": "voice.json" }],
            "modules": [{ "id": "v", "type": "voice" }],
            "connections": []
        })
        .to_string(),
    )
    .unwrap();
    parent.source_path = Some(dir.path().join("parent.json"));
    let error = DevelopmentDefinitions::resolve(&parent)
        .expect_err("a partial range must not load")
        .to_string();
    names(&error, "missing `min`:");
}

#[test]
fn a_file_development_that_cannot_be_read_keeps_its_own_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut parent = Invention::from_json(
        &json!({
            "version": "1.0.0",
            "developments": [{ "name": "voice", "path": "missing.json" }],
            "modules": [{ "id": "v", "type": "voice" }],
            "connections": []
        })
        .to_string(),
    )
    .unwrap();
    parent.source_path = Some(dir.path().join("parent.json"));
    let error = DevelopmentDefinitions::resolve(&parent)
        .expect_err("a missing file must not load")
        .to_string();
    assert!(!error.contains("Development 'voice'"), "{error}");
}

#[test]
fn a_null_bound_is_reported_as_missing() {
    let error = message(Invention::from_json(&inline_document(
        json!({ "min": null, "max": 10 }),
    )));
    names(&error, "missing `min`:");
}

#[test]
fn a_duplicate_bound_in_a_range_is_refused() {
    let control = r#"{ "name": "cutoff", "module": "f", "control": "cutoff",
                       "range": { "min": 0, "min": 1, "max": 10 } }"#;
    let error = serde_json::from_str::<DevelopmentControl>(control)
        .expect_err("a duplicate bound must not load")
        .to_string();
    assert!(error.contains("duplicate field `min`"), "{error}");
}

#[test]
fn a_duplicate_field_in_an_inline_definition_is_refused() {
    let document = r#"{
        "developments": [{ "name": "voice", "definition": {
            "modules": [{ "id": "f", "type": "filter" }],
            "connections": [],
            "connections": [{ "from": "f", "from_port": "out", "to": "f", "to_port": "in" }]
        } }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    }"#;
    let error = message(Invention::from_json(document));
    assert!(error.contains("duplicate field `connections`"), "{error}");
    assert!(error.contains("Development 'voice'"), "{error}");
}

#[test]
fn a_duplicate_field_in_a_definition_written_before_its_name_is_refused() {
    let document = r#"{
        "developments": [{ "definition": {
            "modules": [{ "id": "f", "type": "filter" }],
            "connections": [],
            "connections": [{ "from": "f", "from_port": "out", "to": "f", "to_port": "in" }]
        }, "name": "voice" }],
        "modules": [{ "id": "v", "type": "voice" }],
        "connections": []
    }"#;
    let error = message(Invention::from_json(document));
    assert!(error.contains("duplicate field `connections`"), "{error}");
    assert!(error.contains("Development 'voice'"), "{error}");
}

#[test]
fn a_partial_range_names_the_development_with_the_name_first_in_text() {
    let definition = development(json!({ "max": 10 }));
    let document = format!(
        r#"{{ "developments": [{{ "name": "voice", "definition": {definition} }}],
             "modules": [{{ "id": "v", "type": "voice" }}], "connections": [] }}"#
    );
    names(&message(Invention::from_json(&document)), "missing `min`:");
}

#[test]
fn a_definition_read_in_either_order_parses_the_same() {
    let definition = development(json!({ "min": 0, "max": 10 }));
    let first = format!(
        r#"{{ "developments": [{{ "name": "voice", "definition": {definition} }}],
             "modules": [], "connections": [] }}"#
    );
    let last = format!(
        r#"{{ "developments": [{{ "definition": {definition}, "name": "voice" }}],
             "modules": [], "connections": [] }}"#
    );
    assert_eq!(
        Invention::from_json(&first).unwrap(),
        Invention::from_json(&last).unwrap()
    );
}

#[test]
fn an_error_after_the_name_in_an_inline_definition_reports_one_position() {
    let error = message(Invention::from_json(&inline_document(json!({ "max": 10 }))));
    assert_eq!(error.matches(" at line ").count(), 1, "{error}");
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
        let saved: DevelopmentControl =
            serde_json::from_str(&serde_json::to_string(&control).unwrap()).unwrap();
        assert_eq!(saved, control, "{text}");
    }
    let bare: DevelopmentControl =
        serde_json::from_str(r#"{ "name": "a", "module": "f", "control": "a" }"#).unwrap();
    assert_eq!(bare.range, None);
}
