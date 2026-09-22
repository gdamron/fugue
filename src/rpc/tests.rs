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
fn poll_events_request_and_page_round_trip() {
    use crate::{EventPage, RpcEventPayload, SeqEvent};

    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        request_id: Some("poll".to_string()),
        payload: RpcRequestPayload::PollEvents { after: Some(7) },
    };
    let decoded: RpcRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(
        decoded.payload,
        RpcRequestPayload::PollEvents { after: Some(7) }
    );

    // `after` is optional on the wire.
    let bare: RpcRequest =
        serde_json::from_str(r#"{"schema_version":1,"kind":"poll_events"}"#).unwrap();
    assert_eq!(bare.payload, RpcRequestPayload::PollEvents { after: None });

    let page = EventPage {
        events: vec![SeqEvent {
            seq: 8,
            payload: RpcEventPayload::ControlChanged {
                module_id: "mixer".to_string(),
                key: "master".to_string(),
                value: ControlValue::Number(0.7),
            },
        }],
        latest_seq: 8,
        dropped: false,
    };
    let response = RpcResponse::ok(None, RpcResponsePayload::Events(page.clone()));
    let decoded: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Events(page));
}

#[test]
fn get_meters_request_and_reply_round_trip() {
    use crate::MeterReading;

    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        request_id: Some("meters".to_string()),
        payload: RpcRequestPayload::GetMeters,
    };
    let decoded: RpcRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcRequestPayload::GetMeters);

    let bare: RpcRequest =
        serde_json::from_str(r#"{"schema_version":1,"kind":"get_meters"}"#).unwrap();
    assert_eq!(bare.payload, RpcRequestPayload::GetMeters);

    let meters = vec![MeterReading {
        sink_id: "master".to_string(),
        left_peak: 0.5,
        right_peak: 0.25,
    }];
    let response = RpcResponse::ok(
        None,
        RpcResponsePayload::Meters {
            meters: meters.clone(),
        },
    );
    let decoded: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Meters { meters });
}

#[test]
fn hello_request_round_trips() {
    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        request_id: Some("hello".to_string()),
        payload: RpcRequestPayload::Hello,
    };
    let json = serde_json::to_string(&request).unwrap();
    let decoded: RpcRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.payload, RpcRequestPayload::Hello);
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
fn full_discovery_exposes_metadata_or_an_explicit_error() {
    let registry = ModuleRegistry::default();
    let catalog = ModuleTypeList::from_registry(
        &registry,
        44_100,
        RegistryScope::Builtins,
        &ModuleTypeQuery {
            types: Some(vec!["oscillator".into(), "audio_file_sink".into()]),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    let entries = catalog.details.unwrap();
    assert!(
        matches!(&entries[0], ModuleTypeDetail::Unavailable { type_name, error } if type_name == "audio_file_sink" && error.code == RpcErrorCode::ModuleBuildFailed)
    );
    match &entries[1] {
        ModuleTypeDetail::Available { info } => {
            assert_eq!(info.type_name, "oscillator");
            assert!(info.outputs.contains(&"audio".into()));
            assert!(info.controls.iter().any(|c| c.key == "frequency"));
        }
        _ => panic!("oscillator defaults should be inspectable"),
    }
    let json = serde_json::to_value(&entries[0]).unwrap();
    assert!(json.get("controls").is_none());
    assert!(json.get("inputs").is_none());
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

fn revision(session: &str, revision: u64) -> RuntimeRevision {
    RuntimeRevision {
        session_id: session.to_string(),
        revision,
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

#[test]
fn revision_conflict_error_is_compact_and_structured() {
    let conflict = RevisionConflict {
        expected: revision("session-a", 3),
        current: revision("session-a", 5),
        reason: ConflictReason::StaleRevision,
    };
    let response = RpcResponse::error(
        Some("edit".to_string()),
        RpcError::revision_conflict(conflict.clone()),
    )
    .with_revision(revision("session-a", 5));

    let json = serde_json::to_value(&response).unwrap();
    // The envelope carries the current token alongside the error...
    assert_eq!(json["revision"]["revision"], 5);
    // ...and the error carries both sides plus a machine-readable reason.
    assert_eq!(json["code"], "revision_conflict");
    assert_eq!(json["conflict"]["reason"], "stale_revision");
    assert_eq!(json["conflict"]["expected"]["revision"], 3);
    assert_eq!(json["conflict"]["current"]["revision"], 5);
    assert_eq!(
        serde_json::from_value::<RpcResponse>(json).unwrap(),
        response
    );
}

#[test]
fn ordinary_errors_carry_no_conflict_body() {
    let json = serde_json::to_value(RpcError::unsupported("nope")).unwrap();
    assert!(json.get("conflict").is_none());
}

fn spectrogram_meta() -> SpectrogramStreamMeta {
    SpectrogramStreamMeta {
        stream_id: "master:1".to_string(),
        provenance: SpectrogramProvenance {
            sample_rate: 48_000,
            fft_size: 1024,
            hop_size: 512,
            window: SpectrogramWindow::Hann,
            source: "sink:master".to_string(),
        },
        frequency: SpectrogramFrequencyAxis {
            min_hz: 0.0,
            bin_hz: 46.875,
            bin_count: 513,
        },
        db: SpectrogramDbScale {
            reference: SpectrogramDbReference::Dbfs,
            floor_db: -100.0,
            ceiling_db: 0.0,
        },
        limits: SpectrogramLimits {
            history_frames: 512,
            max_frames_per_tile: 8,
        },
        encoding: SpectrogramEncoding::F32Json,
    }
}

#[test]
fn spectrogram_stream_announcement_round_trips() {
    let event = RpcEvent::new(RpcEventPayload::SpectrogramStream(spectrogram_meta()));
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["event"], "spectrogram_stream");
    assert_eq!(json["schema_version"], RPC_SCHEMA_VERSION);
    assert_eq!(json["stream_id"], "master:1");
    assert_eq!(json["provenance"]["window"], "hann");
    assert_eq!(json["provenance"]["source"], "sink:master");
    assert_eq!(json["frequency"]["bin_count"], 513);
    assert_eq!(json["db"]["reference"], "dbfs");
    assert_eq!(json["limits"]["max_frames_per_tile"], 8);
    // Named from the first stream on, so a compact encoding can be added
    // later without a schema version bump.
    assert_eq!(json["encoding"], "f32_json");

    assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
}

#[test]
fn spectrogram_tile_round_trips() {
    let tile = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 412,
        start_frame: 1173,
        frame_count: 2,
        magnitudes_db: vec![-98.4, -91.2, -72.5, -88.0],
    };
    let event = RpcEvent::new(RpcEventPayload::SpectrogramTile(tile.clone()));
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["event"], "spectrogram_tile");
    assert_eq!(json["tile_seq"], 412);
    assert_eq!(json["start_frame"], 1173);
    assert_eq!(json["frame_count"], 2);
    assert_eq!(json["magnitudes_db"].as_array().unwrap().len(), 4);

    assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
}

#[test]
fn a_tile_must_match_the_shape_its_stream_declared() {
    let meta = SpectrogramStreamMeta {
        frequency: SpectrogramFrequencyAxis {
            bin_count: 2,
            ..spectrogram_meta().frequency
        },
        ..spectrogram_meta()
    };
    let good = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: 2,
        magnitudes_db: vec![-100.0, -90.0, -80.0, -70.0],
    };
    assert!(good.matches(&meta));

    let wrong_stream = SpectrogramTile {
        stream_id: "master:2".to_string(),
        ..good.clone()
    };
    assert!(!wrong_stream.matches(&meta));

    let wrong_length = SpectrogramTile {
        magnitudes_db: vec![-100.0; 3],
        ..good.clone()
    };
    assert!(!wrong_length.matches(&meta));

    let too_many_frames = SpectrogramTile {
        frame_count: meta.limits.max_frames_per_tile + 1,
        ..good.clone()
    };
    assert!(!too_many_frames.matches(&meta));

    let empty = SpectrogramTile {
        frame_count: 0,
        magnitudes_db: Vec::new(),
        ..good
    };
    assert!(!empty.matches(&meta));
}

#[test]
fn spectrogram_topic_names_itself_on_the_wire() {
    let json = serde_json::to_value(RpcSubscriptionTopic::Spectrograms).unwrap();
    assert_eq!(json, "spectrograms");
    assert_eq!(
        serde_json::from_value::<RpcSubscriptionTopic>(json).unwrap(),
        RpcSubscriptionTopic::Spectrograms
    );
}
