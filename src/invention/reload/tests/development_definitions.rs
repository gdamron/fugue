//! Reloading a document whose developments changed on disk or in the
//! document swaps only the instances of the types that changed.

use super::*;

const DEV_BASE: &str = r#"{
    "version": "1.0.0",
    "developments": [
        {
            "name": "voice",
            "definition": {
                "modules": [
                    { "id": "o", "type": "oscillator", "config": { "waveform": "sine", "frequency": 300.0 } }
                ],
                "connections": [],
                "outputs": [ { "name": "audio", "from": "o", "from_port": "audio" } ]
            }
        }
    ],
    "modules": [
        { "id": "v1", "type": "voice" },
        { "id": "v2", "type": "voice" },
        { "id": "solo", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "v1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "v2", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "solo", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn changed_development_definition_swaps_only_its_instances() {
    let mut running = start(DEV_BASE);
    running
        .snapshot()
        .set_control_with_intent(
            "solo",
            "frequency",
            ControlValue::Number(123.0),
            crate::ControlWriteIntent::Perform,
        )
        .unwrap();

    // Only the development's internal definition changes.
    let report = running
        .reload(doc(&DEV_BASE.replace("300.0", "320.0")))
        .expect("diff applies");

    let mut swapped = report.swapped.clone();
    swapped.sort();
    assert_eq!(swapped, vec!["v1", "v2"]);
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    // solo and dac keep playing; solo keeps its live tweak.
    assert_eq!(report.unchanged, 2);
    assert_eq!(
        running.get_control("solo", "frequency").unwrap(),
        ControlValue::Number(123.0)
    );
}

#[test]
fn unchanged_development_definition_leaves_instances_untouched() {
    let _ungoverned = crate::pkg::lock::ungoverned_here();
    let mut running = start(DEV_BASE);

    // Reload the identical document: nothing to do.
    let report = running.reload(doc(DEV_BASE)).expect("diff applies");
    assert!(report.swapped.is_empty());
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    assert!(report.controls_updated.is_empty());
    assert_eq!(report.unchanged, 4);
}

#[test]
fn changed_development_types_are_transitive() {
    let inner = |frequency: f64| {
        format!(
            r#"{{
                "name": "inner",
                "definition": {{
                    "modules": [ {{ "id": "o", "type": "oscillator", "config": {{ "frequency": {frequency} }} }} ],
                    "connections": [],
                    "outputs": [ {{ "name": "audio", "from": "o", "from_port": "audio" }} ]
                }}
            }}"#
        )
    };
    let outer = r#"{
        "name": "outer",
        "definition": {
            "modules": [ { "id": "i", "type": "inner" } ],
            "connections": [],
            "outputs": [ { "name": "audio", "from": "i", "from_port": "audio" } ]
        }
    }"#;
    let document = |frequency: f64| {
        doc(&format!(
            r#"{{
                "version": "1.0.0",
                "developments": [ {}, {} ],
                "modules": [ {{ "id": "dac", "type": "dac" }} ],
                "connections": []
            }}"#,
            inner(frequency),
            outer
        ))
    };

    let previous = DevelopmentDefinitions::resolve(&document(300.0)).unwrap();
    let new = DevelopmentDefinitions::resolve(&document(320.0)).unwrap();

    let changed = changed_development_types(&previous, &new);
    assert!(changed.contains("inner"));
    // outer's own definition is identical, but it instantiates inner.
    assert!(changed.contains("outer"));

    let unchanged = changed_development_types(&previous, &previous.clone());
    assert!(unchanged.is_empty());
}

/// Two siblings each declare a nested `voice` from their own file; editing
/// only the second file rebuilds only the second sibling's instance.
#[test]
fn reload_detects_a_change_to_either_siblings_nested_definition() {
    let _ungoverned = crate::pkg::lock::ungoverned_here();
    let dir = tempfile::tempdir().unwrap();
    let voice = |frequency: f64| {
        serde_json::json!({
            "version": "1.0.0",
            "modules": [{ "id": "osc", "type": "oscillator", "config": { "frequency": frequency } }],
            "connections": [],
            "outputs": [{ "name": "audio", "from": "osc", "from_port": "audio" }]
        })
        .to_string()
    };
    let low_path = dir.path().join("low_voice.json");
    let high_path = dir.path().join("high_voice.json");
    std::fs::write(&low_path, voice(110.0)).unwrap();
    std::fs::write(&high_path, voice(880.0)).unwrap();
    let wrapper = |path: &std::path::Path| {
        serde_json::json!({
            "version": "1.0.0",
            "developments": [{ "name": "voice", "path": path }],
            "modules": [{ "id": "v", "type": "voice" }],
            "connections": [],
            "outputs": [{ "name": "audio", "from": "v", "from_port": "audio" }]
        })
    };
    let root = serde_json::json!({
        "version": "1.0.0",
        "developments": [
            { "name": "low", "definition": wrapper(&low_path) },
            { "name": "high", "definition": wrapper(&high_path) }
        ],
        "modules": [{ "id": "a", "type": "low" }, { "id": "b", "type": "high" }],
        "connections": []
    })
    .to_string();
    let mut running = start(&root);

    std::fs::write(&high_path, voice(990.0)).unwrap();
    let report = running.reload(doc(&root)).expect("diff applies");
    assert_eq!(report.swapped, vec!["b"]);
}
