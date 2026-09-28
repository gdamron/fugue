use super::*;
use crate::ControlValue;

#[test]
fn rpc_commands_round_trip_json() {
    let commands = vec![
        RpcCommand::LoadInvention {
            invention: Box::new(test_invention()),
            source_path: Some("/tmp/inv.json".to_string()),
            frozen: true,
            stop_on_end: true,
            end_source: Some("seq".to_string()),
        },
        RpcCommand::UnloadInvention,
        RpcCommand::SetControl {
            module_id: "osc".to_string(),
            key: "frequency".to_string(),
            value: ControlValue::Number(440.0),
            intent: ControlWriteIntent::Author,
        },
        RpcCommand::SetControls {
            writes: vec![
                ControlWrite::new(
                    "osc".to_string(),
                    "frequency".to_string(),
                    ControlValue::Number(330.0),
                ),
                ControlWrite::new(
                    "mixer".to_string(),
                    "master".to_string(),
                    ControlValue::String("0.7".to_string()),
                ),
            ],
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
        RpcCommand::ListDevelopments {
            query: crate::pkg::content::ContentListQuery::default(),
        },
        RpcCommand::DescribeDevelopment {
            query: crate::pkg::content::ContentDetailQuery {
                schema_version: 1,
                reference: crate::pkg::content::ContentRef::Package {
                    package: "fugue.instruments.pad".into(),
                    version: "0.1.0".into(),
                },
            },
        },
        RpcCommand::ListExamples {
            query: crate::pkg::content::ContentListQuery::default(),
        },
        RpcCommand::DescribeExample {
            query: crate::pkg::content::ContentDetailQuery {
                schema_version: 1,
                reference: crate::pkg::content::ContentRef::Package {
                    package: "fugue.starter.example".into(),
                    version: "1.0.0".into(),
                },
            },
        },
        RpcCommand::LoadExample {
            query: crate::pkg::content::ContentDetailQuery {
                schema_version: 1,
                reference: crate::pkg::content::ContentRef::Package {
                    package: "fugue.starter.example".into(),
                    version: "1.0.0".into(),
                },
            },
            stop_on_end: true,
            end_source: Some("melody".into()),
        },
        RpcCommand::DescribeModuleTypes(ModuleTypeQuery::default()),
        RpcCommand::DescribeModule(DescribeModuleQuery {
            module_type: Some("mixer".into()),
            config: Some(serde_json::json!({"channels":8})),
            ..Default::default()
        }),
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
fn hello_request_round_trips() {
    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        mutation: None,
        request_id: Some("hello".to_string()),
        payload: RpcRequestPayload::Hello,
    };
    let json = serde_json::to_string(&request).unwrap();
    let decoded: RpcRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.payload, RpcRequestPayload::Hello);
}

#[test]
fn shutdown_command_round_trips() {
    let request = RpcRequest::new(RpcCommand::Shutdown).with_request_id("req-1");
    let json = serde_json::to_string(&request).unwrap();
    let decoded: RpcRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(
        decoded.payload,
        RpcRequestPayload::Command(RpcCommand::Shutdown)
    );
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

#[test]
fn control_writes_default_to_authoring_intent() {
    // Clients that predate FUG-266 omit `intent`; their writes must keep
    // authoring (recorded, revision-advancing) exactly as before.
    let single: RpcCommand = serde_json::from_value(serde_json::json!({
        "command": "set_control",
        "module_id": "osc",
        "key": "frequency",
        "value": 440.0,
    }))
    .unwrap();
    assert!(matches!(
        single,
        RpcCommand::SetControl {
            intent: ControlWriteIntent::Author,
            ..
        }
    ));

    let batch: RpcCommand = serde_json::from_value(serde_json::json!({
        "command": "set_controls",
        "writes": [{ "module_id": "osc", "key": "frequency", "value": 330.0 }],
    }))
    .unwrap();
    let RpcCommand::SetControls { writes } = batch else {
        panic!("expected SetControls");
    };
    assert_eq!(writes[0].intent, ControlWriteIntent::Author);
}

#[test]
fn perform_intent_round_trips_on_the_wire() {
    let command = RpcCommand::SetControls {
        writes: vec![ControlWrite::performed(
            "osc",
            "frequency",
            ControlValue::Number(220.0),
        )],
    };
    let json = serde_json::to_value(&command).unwrap();
    assert_eq!(json["writes"][0]["intent"], "perform");
    assert_eq!(serde_json::from_value::<RpcCommand>(json).unwrap(), command);
}

#[test]
fn advances_revision_separates_authoring_from_performance() {
    let author = |intent| RpcCommand::SetControl {
        module_id: "osc".to_string(),
        key: "frequency".to_string(),
        value: ControlValue::Number(440.0),
        intent,
    };
    assert!(author(ControlWriteIntent::Author).advances_revision());
    assert!(!author(ControlWriteIntent::Perform).advances_revision());

    // A batch authors if any one of its writes does.
    let mixed = RpcCommand::SetControls {
        writes: vec![
            ControlWrite::performed("osc", "frequency", ControlValue::Number(1.0)),
            ControlWrite::new("mixer", "master", ControlValue::Number(0.5)),
        ],
    };
    assert!(mixed.advances_revision());
    let performed = RpcCommand::SetControls {
        writes: vec![ControlWrite::performed(
            "osc",
            "frequency",
            ControlValue::Number(1.0),
        )],
    };
    assert!(!performed.advances_revision());

    // Structure always authors; reads and lifecycle never do.
    assert!(RpcCommand::RemoveModule {
        id: "osc".to_string()
    }
    .advances_revision());
    assert!(RpcCommand::ReloadInvention {
        invention: Box::new(test_invention()),
        source_path: None,
        frozen: true,
    }
    .advances_revision());
    assert!(!RpcCommand::ListPackages.advances_revision());
    assert!(!RpcCommand::SaveInvention {
        path: "/tmp/x.json".to_string()
    }
    .advances_revision());
    assert!(!RpcCommand::Shutdown.advances_revision());
}

#[test]
fn unconditional_requests_omit_expected_revision_on_the_wire() {
    let json = serde_json::to_value(RpcRequest::new(RpcCommand::UnloadInvention)).unwrap();
    assert!(json.get("expected_revision").is_none());

    // And a legacy request without the field parses as unconditional.
    let parsed: RpcRequest = serde_json::from_value(serde_json::json!({
        "schema_version": RPC_SCHEMA_VERSION,
        "kind": "command",
        "command": "unload_invention",
    }))
    .unwrap();
    assert_eq!(parsed.expected_revision, None);
}

#[test]
fn preconditioned_request_round_trips() {
    let request = RpcRequest::new(RpcCommand::UnloadInvention).expecting(revision("session-a", 4));
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["expected_revision"]["session_id"], "session-a");
    assert_eq!(json["expected_revision"]["revision"], 4);
    assert_eq!(serde_json::from_value::<RpcRequest>(json).unwrap(), request);
}
