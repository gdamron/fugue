use super::*;

fn status(module_count: usize, connection_count: usize) -> crate::RuntimeStatus {
    crate::RuntimeStatus {
        running: true,
        sample_rate: 48_000,
        module_count,
        connection_count,
        diagnostics: None,
        session: None,
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
        snapshot: Some(RuntimeFullSnapshot {
            status: status(3, 2),
            modules: Vec::new(),
            connections: Vec::new(),
        }),
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
fn identity_response_round_trips_without_schema_version_collision() {
    // The identity carries its own `schema_version`; nesting it (rather than
    // flattening) keeps it from colliding with the response envelope's field.
    let identity = DaemonIdentity {
        schema_version: RPC_SCHEMA_VERSION,
        build: BuildFingerprint {
            crate_version: "2026.6.0".to_string(),
            git_sha: Some("abc123".to_string()),
            dirty: Some(false),
        },
        session_id: "session-xyz".to_string(),
        pid: 4242,
    };
    let response = RpcResponse::ok(
        Some("hello".to_string()),
        RpcResponsePayload::Identity {
            identity: identity.clone(),
        },
    );
    let json = serde_json::to_string(&response).unwrap();
    let decoded: RpcResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Identity { identity });
    assert_eq!(decoded.schema_version, RPC_SCHEMA_VERSION);
}

#[test]
fn a_receipt_request_names_its_reply_and_a_default_one_does_not() {
    let command = RpcCommand::SetControl {
        module_id: "osc".to_string(),
        key: "frequency".to_string(),
        value: crate::ControlValue::Number(440.0),
        intent: ControlWriteIntent::Author,
        source: None,
        priority: None,
    };

    // A default request is byte-for-byte what a client before receipts sent.
    let json = serde_json::to_value(RpcRequest::new(command.clone())).unwrap();
    assert!(json.get("reply").is_none(), "{json}");
    let decoded: RpcRequest = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.reply, MutationReply::Snapshot);

    let json =
        serde_json::to_value(RpcRequest::new(command).with_reply(MutationReply::Receipt)).unwrap();
    assert_eq!(json["reply"], "receipt");
    let decoded: RpcRequest = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.reply, MutationReply::Receipt);
}

#[test]
fn a_receipt_carries_status_and_written_controls_only() {
    let receipt = MutationReceipt {
        status: status(40, 60),
        controls: vec![
            AppliedControl::read_back("osc", "frequency", Some(440.0.into())),
            AppliedControl::read_back("gone", "level", None),
        ],
    };
    let response = RpcResponse::ok(None, RpcResponsePayload::Receipt(receipt.clone()));
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["kind"], "receipt");
    assert_eq!(json["status"]["module_count"], 40);
    assert_eq!(json["controls"][0]["value"], 440.0);
    assert!(json["controls"][1].get("value").is_none(), "{json}");
    assert!(json.get("modules").is_none() && json.get("connections").is_none());
    let decoded: RpcResponse = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Receipt(receipt));

    // A command that wrote no control leaves the list off the wire.
    let structural = RpcResponsePayload::Receipt(MutationReceipt {
        status: status(1, 0),
        controls: Vec::new(),
    });
    let json = serde_json::to_value(RpcResponse::ok(None, structural.clone())).unwrap();
    assert!(json.get("controls").is_none(), "{json}");
    let decoded: RpcResponse = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.payload, structural);
}

#[test]
fn a_reload_answered_with_a_receipt_omits_the_snapshot() {
    let outcome = ReloadOutcome {
        mode: ReloadMode::Diff,
        reason: None,
        report: Some(crate::ReloadReport::default()),
        snapshot: None,
    };
    let json = serde_json::to_value(RpcResponse::ok(
        None,
        RpcResponsePayload::Reload(outcome.clone()),
    ))
    .unwrap();
    assert!(json.get("snapshot").is_none(), "{json}");
    let decoded: RpcResponse = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Reload(outcome));
}

#[test]
fn a_receipt_reads_back_the_controls_a_command_wrote() {
    let value = || crate::ControlValue::Number(1.0);
    let single = RpcCommand::SetControl {
        module_id: "osc".to_string(),
        key: "frequency".to_string(),
        value: value(),
        intent: ControlWriteIntent::Perform,
        source: None,
        priority: None,
    };
    assert_eq!(single.written_controls(), vec![("osc", "frequency")]);

    let batch = RpcCommand::SetControls {
        writes: vec![
            ControlWrite::new("osc", "frequency", value()),
            ControlWrite::new("mix", "level", value()),
            ControlWrite::new("osc", "frequency", value()),
        ],
    };
    assert_eq!(
        batch.written_controls(),
        vec![("osc", "frequency"), ("mix", "level"), ("osc", "frequency")]
    );
    assert!(RpcCommand::RemoveModule { id: "osc".into() }
        .written_controls()
        .is_empty());
}

#[test]
fn an_unknown_reply_shape_decodes_rather_than_failing_the_request() {
    let request: RpcRequest = serde_json::from_value(serde_json::json!({
        "schema_version": RPC_SCHEMA_VERSION,
        "kind": "command",
        "command": "unload_invention",
        "reply": "delta",
    }))
    .unwrap();
    assert_eq!(request.reply, MutationReply::Unknown);
    // It is answered as the default, and never goes back on the wire.
    let wire = serde_json::to_value(&request).unwrap();
    assert!(wire.get("reply").is_none(), "{wire}");
}

#[test]
fn only_snapshot_acknowledged_commands_take_a_reply_shape() {
    assert!(RpcCommand::UnloadInvention.replies_with_snapshot());
    assert!(RpcCommand::RemoveModule { id: "osc".into() }.replies_with_snapshot());
    assert!(!RpcCommand::Shutdown.replies_with_snapshot());
    let apply = RpcCommand::ApplyEdits {
        edits: Vec::new(),
        timing: Default::default(),
    };
    assert!(!apply.replies_with_snapshot());
    assert!(!RpcCommand::DescribeModuleTypes(Default::default()).replies_with_snapshot());
}

#[test]
fn an_acknowledged_control_says_what_became_of_its_request() {
    let pending = AppliedControl {
        request: Some("7".to_string()),
        state: Some(ControlRequestState::Pending),
        requested: Some(crate::ControlValue::Number(330.0)),
        ..AppliedControl::read_back("osc", "frequency", None)
    };
    let json = serde_json::to_value(&pending).unwrap();
    assert_eq!(json["state"], "pending");
    assert_eq!(json["request"], "7");
    assert_eq!(json["requested"], 330.0);
    assert!(json.get("value").is_none() && json.get("reason").is_none());
    let decoded: AppliedControl = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, pending);

    // From a daemon that predates them, none is sent.
    let json = serde_json::json!({ "module_id": "osc", "key": "frequency", "value": 1.0 });
    let decoded: AppliedControl = serde_json::from_value(json).unwrap();
    assert_eq!((decoded.state, decoded.request), (None, None));
}
