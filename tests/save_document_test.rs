//! Lossless save round-trip: the runtime retains the authored document,
//! runtime mutations update it, and saving then reloading reproduces an
//! equivalent graph — developments intact, not flattened.

mod support;

use std::path::{Path, PathBuf};

use fugue::{ControlValue, Invention, InventionBuilder, RunningInvention};
use support::NullAudioBackend;

const SAMPLE_RATE: u32 = 48_000;

const VOICE_DEVELOPMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220.0 } },
        { "id": "amp", "type": "vca", "config": {} }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "amp", "to_port": "audio" }
    ],
    "inputs": [ { "name": "frequency", "to": "osc", "to_port": "frequency" } ],
    "outputs": [ { "name": "audio", "from": "amp", "from_port": "audio" } ],
    "controls": [ { "key": "cv", "module": "amp", "control": "cv" } ]
}"#;

fn invention_with_path_development(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("voice.json"), VOICE_DEVELOPMENT).unwrap();
    let invention = r#"{
        "version": "1.0.0",
        "title": "save round-trip",
        "description": "exercises the retained document",
        "developments": [ { "name": "voice", "path": "voice.json" } ],
        "modules": [
            { "id": "lead", "type": "voice", "config": {} },
            { "id": "dac", "type": "dac", "config": {} }
        ],
        "connections": [
            { "from": "lead", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let path = dir.join("piece.json");
    std::fs::write(&path, invention).unwrap();
    path
}

fn start_from_file(path: &Path) -> RunningInvention {
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    runtime
        .start_with_backend(NullAudioBackend::new(SAMPLE_RATE))
        .unwrap()
}

#[test]
fn unmodified_invention_saves_with_developments_intact() {
    let dir = tempfile::tempdir().unwrap();
    let source = invention_with_path_development(dir.path());
    let running = start_from_file(&source);

    let document = running.document().expect("document retained");
    running.stop();

    assert_eq!(document.title.as_deref(), Some("save round-trip"));
    assert_eq!(
        document.description.as_deref(),
        Some("exercises the retained document")
    );
    assert_eq!(document.developments.len(), 1);
    assert_eq!(document.developments[0].name, "voice");
    assert_eq!(
        document.developments[0].path.as_deref(),
        Some("voice.json"),
        "path-based development stays a path reference"
    );
    // The development instance survives as a module of the development type,
    // not flattened into its internal modules.
    let types: Vec<&str> = document
        .modules
        .iter()
        .map(|module| module.module_type.as_str())
        .collect();
    assert_eq!(types, ["voice", "dac"]);
    assert_eq!(document.connections.len(), 1);
}

#[test]
fn mutated_invention_round_trips_to_an_equivalent_graph() {
    let dir = tempfile::tempdir().unwrap();
    let source = invention_with_path_development(dir.path());
    let running = start_from_file(&source);

    // Mutate live: add a module, wire it, tweak controls — including a
    // development-exposed control.
    running
        .add_module(
            "lfo",
            "lfo",
            &serde_json::json!({ "frequency": 0.5, "waveform": "sine" }),
        )
        .unwrap();
    running.connect("lfo", "out", "lead", "frequency").unwrap();
    running
        .set_control("lead", "cv", ControlValue::Number(0.7))
        .unwrap();

    let document = running.document().expect("document retained");
    running.stop();

    // The mutations landed in the document.
    let lfo = document
        .modules
        .iter()
        .find(|module| module.id == "lfo")
        .expect("added module appears in the document");
    assert_eq!(lfo.config["frequency"], 0.5);
    let lead = document
        .modules
        .iter()
        .find(|module| module.id == "lead")
        .expect("development instance present");
    assert_eq!(lead.config["cv"], 0.7);
    assert!(document
        .connections
        .iter()
        .any(|conn| conn.from == "lfo" && conn.to == "lead"));

    // Save and reload cold: the graph is equivalent and the control change
    // survives through the development instance's config.
    let saved = dir.path().join("saved.json");
    document.save_to_file(&saved).unwrap();
    let reloaded = start_from_file(&saved);

    let snapshot = reloaded.full_snapshot();
    assert_eq!(snapshot.modules.len(), 3);
    assert_eq!(snapshot.connections.len(), 2);
    assert_eq!(
        reloaded.get_control("lead", "cv").unwrap(),
        ControlValue::Number(0.7)
    );
    reloaded.stop();
}

#[test]
fn removed_module_disappears_from_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let source = invention_with_path_development(dir.path());
    let running = start_from_file(&source);

    running.remove_module("lead").unwrap();
    let document = running.document().expect("document retained");
    running.stop();

    assert!(document.modules.iter().all(|module| module.id != "lead"));
    assert!(document.connections.is_empty());
}

#[test]
fn development_config_initializes_exposed_controls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("voice.json"), VOICE_DEVELOPMENT).unwrap();
    let invention = r#"{
        "version": "1.0.0",
        "developments": [ { "name": "voice", "path": "voice.json" } ],
        "modules": [
            { "id": "lead", "type": "voice", "config": { "cv": 0.25 } },
            { "id": "dac", "type": "dac", "config": {} }
        ],
        "connections": [
            { "from": "lead", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let path = dir.path().join("piece.json");
    std::fs::write(&path, invention).unwrap();

    let running = start_from_file(&path);
    assert_eq!(
        running.get_control("lead", "cv").unwrap(),
        ControlValue::Number(0.25)
    );
    running.stop();
}

#[test]
fn development_config_rejects_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("voice.json"), VOICE_DEVELOPMENT).unwrap();
    let invention = r#"{
        "version": "1.0.0",
        "developments": [ { "name": "voice", "path": "voice.json" } ],
        "modules": [ { "id": "lead", "type": "voice", "config": { "nope": 1.0 } } ],
        "connections": []
    }"#;
    let path = dir.path().join("piece.json");
    std::fs::write(&path, invention).unwrap();

    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let err = InventionBuilder::new(SAMPLE_RATE)
        .build(invention)
        .err()
        .expect("unknown development config key is a build error");
    assert!(err.to_string().contains("nope"), "{err}");
}
