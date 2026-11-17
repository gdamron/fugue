use std::path::Path;

use serde_json::{json, Value};

use super::super::test_packs::{
    install, install_pulse_agent, install_pulse_agent_as, CLEAN_AUDIO, MUSICAL_INTENT, PERSONA,
    PULSE_AGENT, PULSE_SKILL,
};
use crate::pkg::audio_asset::with_packs_dir;
use crate::{
    AgentControls, ControlValue, DescribeModuleQuery, Invention, InventionBuilder, ModuleRegistry,
    NullBackend, RunningInvention,
};

const REFERENCE: &str = "fugue.demo.pulse-agent@^0.1.0";

/// Runs `body` with the demo agent installed in a temp packages dir.
fn with_pulse_agent(body: impl FnOnce(&Path)) {
    let packs = tempfile::tempdir().unwrap();
    install_pulse_agent(packs.path());
    with_packs_dir(packs.path(), || body(packs.path()));
}

fn text(value: ControlValue) -> String {
    match value {
        ControlValue::String(text) => text,
        other => panic!("not text: {other:?}"),
    }
}

/// A piece of one agent, `improvise`, with `config`.
fn piece(config: Value) -> Invention {
    let document = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "improvise", "type": "agent", "config": config },
            { "id": "dac", "type": "dac" }
        ],
        "connections": []
    });
    Invention::from_json(&document.to_string()).unwrap()
}

fn start(config: Value) -> (RunningInvention, AgentControls) {
    let (runtime, handles) = InventionBuilder::new(48_000).build(piece(config)).unwrap();
    let controls = handles.get("improvise.controls").unwrap();
    let running = runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap();
    (running, controls)
}

fn load_error(config: Value) -> String {
    match InventionBuilder::new(48_000).build(piece(config)) {
        Ok(_) => panic!("loaded"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_definition_runs_with_the_resolved_prompt_and_shows_its_parts() {
    with_pulse_agent(|packs| {
        let resolved = crate::pkg::resolve_agent(REFERENCE, packs).unwrap();
        assert!(resolved.system_prompt.starts_with(PERSONA));
        let config = json!({ "definition": REFERENCE, "prompt": "Vary the bass line" });
        let (running, _) = start(config);
        let get = |key: &str| text(running.get_control("improvise", key).unwrap());
        assert_eq!(get("system_prompt"), resolved.system_prompt);
        assert_eq!(get("prompt"), "Vary the bass line");
        assert_eq!(get("definition"), REFERENCE);

        let summary: Value = serde_json::from_str(&get("resolved_definition")).unwrap();
        assert_eq!(
            (summary["id"].as_str(), summary["version"].as_str()),
            (Some(PULSE_AGENT), Some("0.1.0"))
        );
        assert_eq!(summary["chars"], resolved.system_prompt.chars().count());
        assert_eq!(summary["model"], Value::Null);
        assert_eq!(summary["tools"], json!(["fugue.set_control"]));
        let part = |source: &str, (package, version): (&str, &str), text: &str| json!({ "source": source, "package": package, "version": version, "chars": text.len() });
        let parts: Vec<Value> = (summary["parts"].as_array().unwrap().iter())
            .map(|p| {
                let keys = ["source", "package", "version", "chars"];
                Value::Object(
                    keys.iter()
                        .map(|k| (k.to_string(), p[*k].clone()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            parts,
            [
                part("persona", (PULSE_AGENT, "0.1.0"), PERSONA),
                part(
                    "policy",
                    (CLEAN_AUDIO, "2026.11.0"),
                    "Keep the audio clean."
                ),
                part(
                    "policy",
                    (MUSICAL_INTENT, "2026.11.0"),
                    "Serve the musical intent."
                ),
                part(
                    "skill",
                    (PULSE_SKILL, "0.1.0"),
                    "Turn a pulse into a phrase."
                ),
            ]
        );

        // Read through the running module's description too.
        let query = DescribeModuleQuery {
            module_id: Some("improvise".into()),
            ..Default::default()
        };
        let described = running.describe_module(&query).unwrap();
        assert_eq!(
            described.control_values["definition"],
            REFERENCE.to_string().into()
        );
        for key in ["definition", "resolved_definition"] {
            let meta = described.info.controls.iter().find(|meta| meta.key == key);
            assert!(meta.is_some_and(|meta| meta.read_only), "{key}");
        }
        running.stop();
    });
}

#[test]
fn describe_module_lists_definition_read_only() {
    let query = DescribeModuleQuery {
        module_type: Some("agent".into()),
        ..Default::default()
    };
    let described = crate::rpc::ModuleDescription::from_registry(
        &ModuleRegistry::default(),
        48_000,
        crate::RegistryScope::Builtins,
        &query,
    )
    .unwrap();
    let listed = |key: &str| described.info.controls.iter().find(|meta| meta.key == key);
    assert!(listed("definition").is_some_and(|meta| meta.read_only));
    assert_eq!(described.control_values["definition"], String::new().into());
    // Only a module running a definition shows what it resolved to.
    assert!(listed("resolved_definition").is_none());

    // Described with a config naming one, it resolves.
    with_pulse_agent(|_| {
        let query = DescribeModuleQuery {
            module_type: Some("agent".into()),
            config: Some(json!({ "definition": REFERENCE })),
            ..Default::default()
        };
        let described = crate::rpc::ModuleDescription::from_registry(
            &ModuleRegistry::default(),
            48_000,
            crate::RegistryScope::Builtins,
            &query,
        )
        .unwrap();
        let resolved = text(described.control_values["resolved_definition"].clone());
        assert!(resolved.contains(PULSE_AGENT), "{resolved}");
    });
}

#[test]
fn a_definition_beside_an_inline_system_prompt_is_refused() {
    with_pulse_agent(|_| {
        let error = load_error(json!({ "definition": REFERENCE, "system_prompt": "Inline." }));
        assert!(error.contains("module 'improvise'"), "{error}");
        assert!(
            error.contains("'definition'") && error.contains("'system_prompt'"),
            "{error}"
        );
        // An empty inline prompt sets nothing, and the task is still given.
        let config = json!({ "definition": REFERENCE, "system_prompt": "", "prompt": "Go" });
        assert!(InventionBuilder::new(48_000).build(piece(config)).is_ok());
    });
}

#[test]
fn a_definition_that_does_not_resolve_refuses_the_load_naming_module_and_package() {
    with_pulse_agent(|packs| {
        let cases = [
            (PULSE_AGENT.to_string(), PULSE_AGENT),
            (
                "fugue.demo.absent@^1".to_string(),
                "fugue.demo.absent is not installed",
            ),
            (format!("{PULSE_SKILL}@^0.1.0"), "`skill` package"),
            (format!("{PULSE_AGENT}@^2"), "satisfies ^2"),
        ];
        for (reference, expected) in cases {
            let error = load_error(json!({ "definition": reference }));
            assert!(error.contains("module 'improvise'"), "{error}");
            assert!(error.contains(expected), "{reference}: {error}");
        }
        let error = load_error(json!({ "definition": 7 }));
        assert!(error.contains("module 'improvise'"), "{error}");
        assert!(error.contains("'definition'"), "{error}");
        // A policy it requires is missing.
        std::fs::remove_dir_all(packs.join(MUSICAL_INTENT)).unwrap();
        let error = load_error(json!({ "definition": REFERENCE }));
        assert!(
            error.contains("module 'improvise'") && error.contains(MUSICAL_INTENT),
            "{error}"
        );
    });
    // Its definition file is not a definition.
    let broken = tempfile::tempdir().unwrap();
    install_pulse_agent_as(broken.path(), "{ not json");
    with_packs_dir(broken.path(), || {
        let error = load_error(json!({ "definition": REFERENCE }));
        assert!(error.contains("module 'improvise'"), "{error}");
        assert!(error.contains(PULSE_AGENT), "{error}");
    });
}

#[path = "definition_play_tests.rs"]
mod play;
