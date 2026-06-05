use super::*;
use crate::{
    AssetSpec, ControlValue, DevelopmentControl, DevelopmentInput, DevelopmentOutput,
    DevelopmentSpec,
};
use std::collections::BTreeMap;

fn voice_development() -> Invention {
    Invention {
        version: "1.0.0".to_string(),
        title: Some("voice".to_string()),
        description: None,
        developments: vec![],
        assets: BTreeMap::new(),
        modules: vec![
            crate::ModuleSpec {
                id: "osc".to_string(),
                module_type: "oscillator".to_string(),
                config: serde_json::json!({"frequency": 220.0}),
            },
            crate::ModuleSpec {
                id: "vca".to_string(),
                module_type: "vca".to_string(),
                config: serde_json::json!({"level": 1.0}),
            },
        ],
        connections: vec![crate::Connection {
            from: "osc".to_string(),
            to: "vca".to_string(),
            from_port: Some("audio".to_string()),
            to_port: Some("audio".to_string()),
        }],
        inputs: vec![DevelopmentInput {
            name: "frequency".to_string(),
            to: "osc".to_string(),
            to_port: "frequency".to_string(),
        }],
        outputs: vec![DevelopmentOutput {
            name: "audio".to_string(),
            from: "vca".to_string(),
            from_port: "audio".to_string(),
        }],
        controls: vec![DevelopmentControl {
            key: "type".to_string(),
            module: "osc".to_string(),
            control: "type".to_string(),
        }],
        source_path: None,
    }
}

fn root_invention_with_voice(voice: DevelopmentSpec) -> Invention {
    Invention {
        version: "1.0.0".to_string(),
        title: Some("root".to_string()),
        description: None,
        developments: vec![voice],
        assets: BTreeMap::new(),
        modules: vec![
            crate::ModuleSpec {
                id: "lead".to_string(),
                module_type: "voice".to_string(),
                config: serde_json::Value::Null,
            },
            crate::ModuleSpec {
                id: "dac".to_string(),
                module_type: "dac".to_string(),
                config: serde_json::Value::Null,
            },
        ],
        connections: vec![crate::Connection {
            from: "lead".to_string(),
            to: "dac".to_string(),
            from_port: Some("audio".to_string()),
            to_port: Some("audio".to_string()),
        }],
        inputs: vec![],
        outputs: vec![],
        controls: vec![],
        source_path: None,
    }
}

#[test]
fn builds_inline_development_as_module() {
    let invention = root_invention_with_voice(DevelopmentSpec {
        name: "voice".to_string(),
        path: None,
        definition: Some(Box::new(voice_development())),
    });

    let builder = InventionBuilder::new(44_100);
    let (runtime, _) = builder.build(invention).unwrap();

    let module = runtime.modules.get("lead").unwrap().module();
    assert!(module.inputs().contains(&"frequency"));
    assert!(module.outputs().contains(&"audio"));

    let controls = runtime.control_surfaces.get("lead").unwrap().controls();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].key, "type");
}

#[test]
fn development_controls_alias_internal_surface() {
    let invention = root_invention_with_voice(DevelopmentSpec {
        name: "voice".to_string(),
        path: None,
        definition: Some(Box::new(voice_development())),
    });

    let builder = InventionBuilder::new(44_100);
    let (runtime, _) = builder.build(invention).unwrap();
    let surface = runtime.control_surfaces.get("lead").unwrap();

    surface
        .set_control("type", ControlValue::String("square".to_string()))
        .unwrap();
    assert_eq!(
        surface.get_control("type").unwrap(),
        ControlValue::String("square".to_string())
    );
}

#[test]
fn development_fans_out_exposed_inputs_and_caches_outputs() {
    let development = Invention {
        version: "1.0.0".to_string(),
        title: Some("fanout".to_string()),
        description: None,
        developments: vec![],
        assets: BTreeMap::new(),
        modules: vec![
            crate::ModuleSpec {
                id: "full".to_string(),
                module_type: "vca".to_string(),
                config: serde_json::json!({"cv": 1.0}),
            },
            crate::ModuleSpec {
                id: "half".to_string(),
                module_type: "vca".to_string(),
                config: serde_json::json!({"cv": 0.5}),
            },
        ],
        connections: vec![],
        inputs: vec![
            DevelopmentInput {
                name: "signal".to_string(),
                to: "full".to_string(),
                to_port: "audio".to_string(),
            },
            DevelopmentInput {
                name: "signal".to_string(),
                to: "half".to_string(),
                to_port: "audio".to_string(),
            },
        ],
        outputs: vec![
            DevelopmentOutput {
                name: "full".to_string(),
                from: "full".to_string(),
                from_port: "audio".to_string(),
            },
            DevelopmentOutput {
                name: "half".to_string(),
                from: "half".to_string(),
                from_port: "audio".to_string(),
            },
        ],
        controls: vec![],
        source_path: None,
    };

    let root = Invention {
        version: "1.0.0".to_string(),
        title: Some("root".to_string()),
        description: None,
        developments: vec![DevelopmentSpec {
            name: "fanout".to_string(),
            path: None,
            definition: Some(Box::new(development)),
        }],
        assets: BTreeMap::new(),
        modules: vec![crate::ModuleSpec {
            id: "voice".to_string(),
            module_type: "fanout".to_string(),
            config: serde_json::Value::Null,
        }],
        connections: vec![],
        inputs: vec![],
        outputs: vec![],
        controls: vec![],
        source_path: None,
    };

    let (mut runtime, _) = InventionBuilder::new(44_100).build(root).unwrap();
    let voice = runtime.modules.get_mut("voice").unwrap().module_mut();

    voice.set_input("signal", 0.8).unwrap();
    voice.process(1);

    assert_eq!(voice.get_output("full").unwrap(), 0.8);
    assert_eq!(voice.get_output("half").unwrap(), 0.4);
}

#[test]
fn resolves_relative_development_paths() {
    let unique = format!(
        "fugue-development-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).unwrap();

    let development_path = dir.join("voice.json");
    std::fs::write(&development_path, voice_development().to_json().unwrap()).unwrap();

    let root = root_invention_with_voice(DevelopmentSpec {
        name: "voice".to_string(),
        path: Some("voice.json".to_string()),
        definition: None,
    });
    let root_path = dir.join("root.json");
    std::fs::write(&root_path, root.to_json().unwrap()).unwrap();

    let invention = Invention::from_file(&root_path.to_string_lossy()).unwrap();
    let builder = InventionBuilder::new(44_100);
    let (runtime, _) = builder.build(invention).unwrap();

    assert!(runtime.modules.contains_key("lead"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn resolves_relative_asset_refs_in_module_configs() {
    let unique = format!(
        "fugue-asset-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(
        dir.join("score.json"),
        r#"{"base_note_hint":48,"cells":[[{"note":0},{"note":null}],[{"note":12}]]}"#,
    )
    .unwrap();

    let mut assets = BTreeMap::new();
    assets.insert(
        "score".to_string(),
        AssetSpec {
            path: "score.json".to_string(),
        },
    );
    let root = Invention {
        version: "1.0.0".to_string(),
        title: Some("asset root".to_string()),
        description: None,
        developments: vec![],
        assets,
        modules: vec![crate::ModuleSpec {
            id: "seq".to_string(),
            module_type: "cell_sequencer".to_string(),
            config: serde_json::json!({
                "base_note": { "$asset": "score", "path": "/base_note_hint" },
                "sequences": { "$asset": "score", "path": "/cells" },
                "metadata": [{ "source": { "$asset": "score", "path": "/base_note_hint" } }]
            }),
        }],
        connections: vec![],
        inputs: vec![],
        outputs: vec![],
        controls: vec![],
        source_path: None,
    };
    let root_path = dir.join("root.json");
    std::fs::write(&root_path, root.to_json().unwrap()).unwrap();

    let invention = Invention::from_file(&root_path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(44_100).build(invention).unwrap();
    let state = runtime.state.lock().unwrap();
    let config = &state.modules.get("seq").unwrap().config;

    assert_eq!(config["base_note"], 48);
    assert_eq!(config["sequences"].as_array().unwrap().len(), 2);
    assert_eq!(config["metadata"][0]["source"], 48);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn text_assets_resolve_as_string_values() {
    let unique = format!(
        "fugue-text-asset-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).unwrap();

    let script_body = "function init() {}\nreturn { init };\n";
    let prompt_body = "# Conductor\n\nReturn JSON only.\n";
    std::fs::write(dir.join("voice.js"), script_body).unwrap();
    std::fs::write(dir.join("prompt.md"), prompt_body).unwrap();

    let mut assets = BTreeMap::new();
    assets.insert(
        "voice_script".to_string(),
        AssetSpec {
            path: "voice.js".to_string(),
        },
    );
    assets.insert(
        "conductor_prompt".to_string(),
        AssetSpec {
            path: "prompt.md".to_string(),
        },
    );
    let root = Invention {
        version: "1.0.0".to_string(),
        title: Some("text assets".to_string()),
        description: None,
        developments: vec![],
        assets,
        modules: vec![
            crate::ModuleSpec {
                id: "voice".to_string(),
                module_type: "code".to_string(),
                config: serde_json::json!({
                    "script": { "$asset": "voice_script" },
                    "tick_hz": 0.0,
                    "enabled": false
                }),
            },
            crate::ModuleSpec {
                id: "conductor".to_string(),
                module_type: "agent".to_string(),
                config: serde_json::json!({
                    "prompt": { "$asset": "conductor_prompt" },
                    "enabled": false
                }),
            },
        ],
        connections: vec![],
        inputs: vec![],
        outputs: vec![],
        controls: vec![],
        source_path: None,
    };
    let root_path = dir.join("root.json");
    std::fs::write(&root_path, root.to_json().unwrap()).unwrap();

    let invention = Invention::from_file(&root_path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(44_100).build(invention).unwrap();
    let state = runtime.state.lock().unwrap();

    let voice_config = &state.modules.get("voice").unwrap().config;
    assert_eq!(voice_config["script"], serde_json::Value::String(script_body.to_string()));

    let conductor_config = &state.modules.get("conductor").unwrap().config;
    assert_eq!(
        conductor_config["prompt"],
        serde_json::Value::String(prompt_body.to_string())
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn asset_ref_errors_on_missing_pointer() {
    let mut assets = HashMap::new();
    assets.insert("score".to_string(), serde_json::json!({"cells": []}));

    let error = resolve_asset_refs(
        &serde_json::json!({ "$asset": "score", "path": "/missing" }),
        &assets,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("does not contain JSON Pointer '/missing'"));
}

#[test]
fn supports_nested_developments() {
    let inner = Invention {
        version: "1.0.0".to_string(),
        title: Some("inner".to_string()),
        description: None,
        developments: vec![],
        assets: BTreeMap::new(),
        modules: vec![crate::ModuleSpec {
            id: "osc".to_string(),
            module_type: "oscillator".to_string(),
            config: serde_json::json!({"frequency": 440.0}),
        }],
        connections: vec![],
        inputs: vec![],
        outputs: vec![DevelopmentOutput {
            name: "audio".to_string(),
            from: "osc".to_string(),
            from_port: "audio".to_string(),
        }],
        controls: vec![],
        source_path: None,
    };
    let outer = Invention {
        version: "1.0.0".to_string(),
        title: Some("outer".to_string()),
        description: None,
        developments: vec![DevelopmentSpec {
            name: "inner_voice".to_string(),
            path: None,
            definition: Some(Box::new(inner)),
        }],
        assets: BTreeMap::new(),
        modules: vec![crate::ModuleSpec {
            id: "voice".to_string(),
            module_type: "inner_voice".to_string(),
            config: serde_json::Value::Null,
        }],
        connections: vec![],
        inputs: vec![],
        outputs: vec![DevelopmentOutput {
            name: "audio".to_string(),
            from: "voice".to_string(),
            from_port: "audio".to_string(),
        }],
        controls: vec![],
        source_path: None,
    };
    let root = root_invention_with_voice(DevelopmentSpec {
        name: "voice".to_string(),
        path: None,
        definition: Some(Box::new(outer)),
    });

    let builder = InventionBuilder::new(44_100);
    let (runtime, _) = builder.build(root).unwrap();

    assert!(runtime.modules.contains_key("lead"));
}
