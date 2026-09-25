use super::*;

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
