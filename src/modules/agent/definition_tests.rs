use std::path::Path;
use std::time::{Duration, Instant};

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

#[test]
fn a_reload_or_edit_that_does_not_resolve_leaves_the_piece_playing() {
    with_pulse_agent(|_| {
        let (mut running, _) = start(json!({ "definition": REFERENCE }));
        let prompt = running.get_control("improvise", "system_prompt").unwrap();
        let error = running
            .reload(piece(json!({ "definition": "fugue.demo.absent@^1" })))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("improvise") && error.contains("fugue.demo.absent"),
            "{error}"
        );
        assert_eq!(
            running.get_control("improvise", "system_prompt").unwrap(),
            prompt
        );
        assert_eq!(
            text(running.get_control("improvise", "definition").unwrap()),
            REFERENCE
        );

        let edit = crate::rpc::StructuralEdit::AddModule {
            id: "second".into(),
            module_type: "agent".into(),
            config: json!({ "definition": "fugue.demo.absent@^1" }),
            name: None,
        };
        let error = running.apply_edits(&[edit]).unwrap_err();
        assert!(
            format!("{error:?}").contains("fugue.demo.absent"),
            "{error:?}"
        );
        assert!(running.get_control("second", "definition").is_err());

        // Another reference that resolves rebuilds the module with it.
        let exact = format!("{PULSE_AGENT}@=0.1.0");
        running
            .reload(piece(json!({ "definition": exact })))
            .unwrap();
        assert_eq!(
            text(running.get_control("improvise", "definition").unwrap()),
            exact
        );
        assert_eq!(
            running.get_control("improvise", "system_prompt").unwrap(),
            prompt
        );
        running.stop();
    });
}

/// The model the agent's first request named, as its history records it.
fn requested_model(config: Value) -> Value {
    let mut config = config;
    config["backend"] = json!("test:echo");
    let (running, controls) = start(config);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        controls.increment_trigger();
        let count = running.get_control("improvise", "request_count").unwrap();
        if count.as_number().unwrap() >= 1.0 {
            break;
        }
        assert!(Instant::now() < deadline, "the agent made no request");
        std::thread::sleep(Duration::from_millis(20));
    }
    let history = text(running.get_control("improvise", "history").unwrap());
    running.stop();
    serde_json::from_str::<Value>(&history).unwrap()[0]["response"]["model"].clone()
}

#[test]
fn the_definitions_model_is_used_unless_the_config_names_one() {
    let packs = tempfile::tempdir().unwrap();
    let definition = json!({ "system_prompt": PERSONA, "default_model": "pulse-model" });
    install_pulse_agent_as(packs.path(), &definition.to_string());
    with_packs_dir(packs.path(), || {
        assert_eq!(
            requested_model(json!({ "definition": REFERENCE })),
            "pulse-model"
        );
        let unnamed = json!({ "definition": REFERENCE, "model": null });
        assert_eq!(requested_model(unnamed), "pulse-model");
        let named = json!({ "definition": REFERENCE, "model": "named-model" });
        assert_eq!(requested_model(named), "named-model");
        // With neither, the backend chooses.
        assert_eq!(requested_model(json!({})), Value::Null);
    });
}

#[test]
fn a_live_prompt_write_plays_until_the_next_reload() {
    with_pulse_agent(|_| {
        let config = json!({ "definition": REFERENCE });
        let (mut running, _) = start(config.clone());
        let resolved = running.get_control("improvise", "system_prompt").unwrap();
        running
            .set_control(
                "improvise",
                "system_prompt",
                ControlValue::String("Live.".into()),
            )
            .unwrap();
        assert_eq!(
            text(running.get_control("improvise", "system_prompt").unwrap()),
            "Live."
        );
        // The document still loads: it holds the definition, not the write.
        let document = running.document().unwrap();
        let spec = document
            .modules
            .iter()
            .find(|spec| spec.id == "improvise")
            .unwrap();
        assert_eq!(spec.config, config);
        running.reload(piece(config)).unwrap();
        assert_eq!(
            running.get_control("improvise", "system_prompt").unwrap(),
            resolved
        );
        running.stop();

        // Beside an empty inline prompt, a reload restores the definition's
        // too.
        let config = json!({ "definition": REFERENCE, "system_prompt": "" });
        let (mut running, _) = start(config.clone());
        let live = ControlValue::String("Live.".into());
        running
            .set_control("improvise", "system_prompt", live)
            .unwrap();
        running.reload(piece(config)).unwrap();
        assert_eq!(
            running.get_control("improvise", "system_prompt").unwrap(),
            resolved
        );

        // An edit batch cannot author one beside the definition, and says
        // to write it live.
        let edit = crate::rpc::StructuralEdit::SetControl {
            module_id: "improvise".into(),
            key: "system_prompt".into(),
            value: ControlValue::String("Edited.".into()),
        };
        let error = format!("{:?}", running.apply_edits(&[edit]).unwrap_err());
        assert!(error.contains("write the control live"), "{error}");
        assert_eq!(
            running.get_control("improvise", "system_prompt").unwrap(),
            resolved
        );
        running.stop();
    });
}

#[test]
fn a_governing_lock_picks_the_locked_packages_and_a_reload_under_it_rebuilds() {
    use crate::pkg::lock::{govern_here, with_governing, FrozenLock};
    use crate::pkg::{compute_integrity, LockSource, LockedPackage, Lockfile};
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    install_pulse_agent(&packs);
    let mut lockfile = Lockfile::new();
    for (id, version, kind) in [
        (PULSE_AGENT, "0.1.0", "agent"),
        (PULSE_SKILL, "0.1.0", "skill"),
        (CLEAN_AUDIO, "2026.11.0", "policy"),
        (MUSICAL_INTENT, "2026.11.0", "policy"),
    ] {
        let dir = packs.join(id).join(version);
        let source = LockSource::Registry {
            id: id.into(),
            version: version.into(),
        };
        let integrity = compute_integrity(&dir).unwrap();
        let locked = LockedPackage {
            version: version.into(),
            kind: kind.into(),
            source,
            integrity,
            path: dir,
            dependencies: Vec::new(),
        };
        lockfile.upsert(id, locked);
    }
    let path = tmp.path().join(crate::pkg::LOCKFILE_NAME);
    std::fs::write(&path, lockfile.to_bytes().unwrap()).unwrap();
    // A newer skill the lock does not pin.
    let skill = "---\nname: pulse\ndescription: Shape a pulse.\n---\n\nUnlocked newer pulse.\n";
    let policies = json!([format!("{CLEAN_AUDIO}@^2026.11.0")]);
    install(
        &packs,
        (PULSE_SKILL, "0.1.9", "skill"),
        json!({ "policies": policies }),
        &[("SKILL.md", skill)],
    );
    let lock = FrozenLock::find(&[path], &packs).unwrap().unwrap();

    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let prompt = |running: &RunningInvention| {
                text(running.get_control("improvise", "system_prompt").unwrap())
            };
            let config = json!({ "definition": REFERENCE });
            // Built under the session's lock, the locked skill plays.
            let (running, _) = start(config.clone());
            assert!(
                prompt(&running).contains("Turn a pulse into a phrase."),
                "{}",
                prompt(&running)
            );
            running.stop();
            // Built with nothing governing, the newer one does; a reload of
            // the same document under the lock rebuilds it.
            let (mut running, _) = govern_here(None, || start(config.clone()));
            assert!(
                prompt(&running).contains("Unlocked newer pulse."),
                "{}",
                prompt(&running)
            );
            let lock = crate::pkg::lock::governing();
            running.reload_governed(piece(config), lock).unwrap();
            assert!(
                prompt(&running).contains("Turn a pulse into a phrase."),
                "{}",
                prompt(&running)
            );
            running.stop();
            // A requirement only an unlocked version satisfies is refused.
            install(
                &packs,
                (PULSE_AGENT, "0.2.0", "agent"),
                json!({}),
                &[(
                    "agent.json",
                    &json!({ "system_prompt": PERSONA }).to_string(),
                )],
            );
            let error = load_error(json!({ "definition": format!("{PULSE_AGENT}@^0.2.0") }));
            assert!(error.contains("module 'improvise'"), "{error}");
            assert!(error.contains(PULSE_AGENT), "{error}");
        });
    });
}

#[test]
fn the_example_loads_with_the_demo_agent_installed() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/agent_definition_pulse.json");
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let spec = invention
        .modules
        .iter()
        .find(|spec| spec.module_type == "agent")
        .unwrap();
    assert_eq!(spec.config["definition"], REFERENCE);
    with_pulse_agent(|_| {
        let mut engine = crate::RenderEngine::new(48_000);
        engine.stub_agent_backends(true);
        engine.load_invention(invention.clone()).unwrap();
        let mut block = vec![0.0f32; 1024];
        engine.render_interleaved(&mut block).unwrap();
        let prompt = text(engine.get_control(&spec.id, "system_prompt").unwrap());
        assert!(prompt.starts_with(PERSONA), "{prompt}");
    });
    // Without the package, the load is refused naming it.
    let empty = tempfile::tempdir().unwrap();
    with_packs_dir(empty.path(), || {
        let error = InventionBuilder::new(48_000)
            .build(invention)
            .err()
            .unwrap();
        assert!(error.to_string().contains(PULSE_AGENT), "{error}");
    });
}

#[path = "definition_play_tests.rs"]
mod play;
