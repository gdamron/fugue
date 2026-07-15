use super::*;
use crate::{ControlValue, ModuleSpec};

fn test_invention() -> Invention {
    Invention {
        version: "1.0.0".to_string(),
        title: Some("rpc-test".to_string()),
        description: None,
        developments: Vec::new(),
        assets: std::collections::BTreeMap::new(),
        modules: vec![ModuleSpec {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::Value::Null,
        }],
        connections: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        controls: Vec::new(),
        source_path: None,
    }
}

#[test]
fn rpc_commands_round_trip_json() {
    let commands = vec![
        RpcCommand::LoadInvention {
            invention: Box::new(test_invention()),
            frozen: true,
            stop_on_end: true,
            end_source: Some("seq".to_string()),
        },
        RpcCommand::UnloadInvention,
        RpcCommand::SetControl {
            module_id: "osc".to_string(),
            key: "frequency".to_string(),
            value: ControlValue::Number(440.0),
        },
        RpcCommand::AddModule {
            id: "osc".to_string(),
            module_type: "oscillator".to_string(),
            config: serde_json::json!({ "frequency": 440.0 }),
        },
        RpcCommand::RemoveModule {
            id: "osc".to_string(),
        },
        RpcCommand::Connect {
            from: "osc".to_string(),
            from_port: "audio".to_string(),
            to: "dac".to_string(),
            to_port: "audio".to_string(),
        },
        RpcCommand::Disconnect {
            from: "osc".to_string(),
            from_port: "audio".to_string(),
            to: "dac".to_string(),
            to_port: "audio".to_string(),
        },
        RpcCommand::SwapModule {
            id: "osc".to_string(),
            module_type: "lfo".to_string(),
            config: serde_json::json!({ "frequency": 2.0 }),
            preserve_connections: true,
        },
        RpcCommand::ReloadInvention {
            invention: Box::new(test_invention()),
            source_path: Some("/tmp/invention.json".to_string()),
            frozen: true,
        },
        RpcCommand::InstallPackage(PackageInstallRequest {
            package: "demo".to_string(),
            version: Some("1.2.3".to_string()),
        }),
        RpcCommand::ListPackages,
        RpcCommand::DescribeModuleTypes,
    ];

    for command in commands {
        let request = RpcRequest::new(command.clone()).with_request_id("req-1");
        let json = serde_json::to_string(&request).unwrap();
        let decoded: RpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, RPC_SCHEMA_VERSION);
        assert_eq!(decoded.payload, RpcRequestPayload::Command(command));
    }
}

#[test]
fn reload_command_defaults_and_outcome_round_trip() {
    // A client omitting the optional fields still decodes (wire back-compat).
    let decoded: RpcCommand = serde_json::from_str(
        r#"{ "command": "reload_invention",
             "invention": { "modules": [], "connections": [] } }"#,
    )
    .unwrap();
    match decoded {
        RpcCommand::ReloadInvention {
            source_path,
            frozen,
            ..
        } => {
            assert_eq!(source_path, None);
            assert!(frozen, "frozen defaults to lockfile validation on");
        }
        other => panic!("expected reload command, got {other:?}"),
    }

    let outcome = ReloadOutcome {
        mode: ReloadMode::Diff,
        reason: None,
        report: Some(crate::ReloadReport {
            added: vec!["osc".to_string()],
            controls_updated: vec!["osc.frequency".to_string()],
            unchanged: 2,
            ..Default::default()
        }),
        snapshot: RuntimeFullSnapshot {
            status: crate::RuntimeStatus {
                running: true,
                sample_rate: 48_000,
                module_count: 3,
                connection_count: 2,
                diagnostics: None,
            },
            modules: Vec::new(),
            connections: Vec::new(),
        },
    };
    let response = RpcResponse::ok(
        Some("req-1".to_string()),
        RpcResponsePayload::Reload(outcome.clone()),
    );
    let json = serde_json::to_string(&response).unwrap();
    let decoded: RpcResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Reload(outcome));
}

#[test]
fn schema_version_rejects_incompatible_clients() {
    let error = validate_schema_version(RPC_SCHEMA_VERSION + 1).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::IncompatibleSchemaVersion);
    assert!(error.message.contains("incompatible RPC schema version"));
}

#[test]
fn graph_errors_map_to_rpc_errors() {
    let error = RpcError::from(GraphCommandError::UnknownModule("osc".to_string()));
    assert_eq!(error.code, RpcErrorCode::UnknownModule);
    assert!(error.message.contains("osc"));
}

#[test]
fn built_in_packages_list_registry_types() {
    let registry = ModuleRegistry::default();
    let packages = PackageList::built_in(&registry);
    assert_eq!(packages.packages.len(), 1);
    assert_eq!(packages.packages[0].source, PackageSource::BuiltIn);
    assert!(packages.packages[0]
        .module_types
        .contains(&"oscillator".to_string()));
}

#[test]
fn built_in_module_types_include_ports_and_controls() {
    let registry = ModuleRegistry::default();
    let module_types = ModuleTypeList::built_in(&registry, 44_100);
    let oscillator = module_types
        .module_types
        .iter()
        .find(|module_type| module_type.type_name == "oscillator")
        .expect("oscillator module type is listed");
    assert!(oscillator.outputs.contains(&"audio".to_string()));
    assert!(oscillator
        .controls
        .iter()
        .any(|control| control.key == "frequency"));

    #[cfg(not(target_arch = "wasm32"))]
    {
        let audio_file_sink = module_types
            .module_types
            .iter()
            .find(|module_type| module_type.type_name == "audio_file_sink")
            .expect("audio_file_sink module type is listed");
        assert!(audio_file_sink.is_sink);
        assert!(audio_file_sink.inputs.contains(&"audio".to_string()));
        assert!(audio_file_sink.outputs.contains(&"audio_left".to_string()));
    }
}

#[test]
fn package_install_placeholder_is_structured_unsupported_error() {
    let error = RpcError::unsupported("package installation is not implemented yet");
    assert_eq!(error.code, RpcErrorCode::Unsupported);
    assert!(error.message.contains("not implemented"));
}

#[test]
fn render_engine_full_snapshot_includes_ports_and_control_values() {
    let json = r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let mut engine = crate::RenderEngine::new(44_100);
    engine.load_json(json).unwrap();
    engine
        .set_control("osc", "frequency", ControlValue::Number(880.0))
        .unwrap();

    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.status.module_count, 2);
    assert_eq!(snapshot.connections.len(), 1);

    let osc = snapshot
        .modules
        .iter()
        .find(|module| module.info.id == "osc")
        .expect("oscillator module is present");
    assert!(osc.ports.outputs.contains(&"audio".to_string()));
    assert!(osc.ports.inputs.contains(&"frequency".to_string()));
    let frequency = osc
        .controls
        .iter()
        .find(|control| control.meta.key == "frequency")
        .expect("frequency control is present");
    assert_eq!(frequency.value, Some(ControlValue::Number(880.0)));
}

#[cfg(feature = "rpc-schema")]
#[test]
fn runtime_rpc_schema_generates() {
    let schema = schema::runtime_rpc_schema();
    let json = serde_json::to_value(schema).unwrap();
    assert!(json.is_object());
}

#[test]
fn load_invention_defaults_stop_on_end_fields() {
    // Older clients omit the stop-on-end fields; they must default off.
    let json = serde_json::json!({
        "command": "load_invention",
        "invention": serde_json::to_value(test_invention()).unwrap(),
    });
    let command: RpcCommand = serde_json::from_value(json).unwrap();
    match command {
        RpcCommand::LoadInvention {
            frozen,
            stop_on_end,
            end_source,
            ..
        } => {
            assert!(frozen, "frozen defaults on");
            assert!(!stop_on_end, "stop_on_end defaults off");
            assert_eq!(end_source, None);
        }
        other => panic!("expected LoadInvention, got {:?}", other),
    }
}
