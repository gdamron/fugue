use super::*;
use crate::ControlValue;
use serde_json::json;

fn add(id: &str) -> StructuralEdit {
    StructuralEdit::AddModule {
        id: id.to_string(),
        module_type: "oscillator".to_string(),
        config: json!({ "frequency": 220.0 }),
    }
}

fn apply(edits: Vec<StructuralEdit>) -> RpcCommand {
    RpcCommand::ApplyEdits { edits }
}

fn ticket(id: &str, issued_at: RuntimeRevision) -> MutationTicket {
    MutationTicket {
        id: id.to_string(),
        issued_at,
    }
}

fn round_trip(edit: StructuralEdit, expected: serde_json::Value) {
    let json = serde_json::to_value(&edit).unwrap();
    assert_eq!(json, expected);
    assert_eq!(
        serde_json::from_value::<StructuralEdit>(json).unwrap(),
        edit
    );
}

#[test]
fn every_op_has_a_flat_tagged_wire_shape() {
    round_trip(
        add("lfo"),
        json!({ "op": "add_module", "id": "lfo", "module_type": "oscillator",
                "config": { "frequency": 220.0 } }),
    );
    round_trip(
        StructuralEdit::RemoveModule { id: "lfo".into() },
        json!({ "op": "remove_module", "id": "lfo" }),
    );
    let wire = |op: &str| json!({ "op": op, "from": "lfo", "from_port": "audio", "to": "osc", "to_port": "fm" });
    round_trip(
        StructuralEdit::Connect {
            from: "lfo".into(),
            from_port: "audio".into(),
            to: "osc".into(),
            to_port: "fm".into(),
        },
        wire("connect"),
    );
    round_trip(
        StructuralEdit::Disconnect {
            from: "lfo".into(),
            from_port: "audio".into(),
            to: "osc".into(),
            to_port: "fm".into(),
        },
        wire("disconnect"),
    );
    round_trip(
        StructuralEdit::SetControl {
            module_id: "osc".into(),
            key: "type".into(),
            value: ControlValue::String("saw".into()),
        },
        json!({ "op": "set_control", "module_id": "osc", "key": "type", "value": "saw" }),
    );
}

#[test]
fn add_module_config_defaults_to_null() {
    let edit: StructuralEdit =
        serde_json::from_value(json!({ "op": "add_module", "id": "a", "module_type": "dac" }))
            .unwrap();
    assert_eq!(
        edit,
        StructuralEdit::AddModule {
            id: "a".into(),
            module_type: "dac".into(),
            config: serde_json::Value::Null,
        }
    );
}

#[test]
fn unknown_fields_and_ops_are_refused() {
    // set_control is always authoring; an intent field is a mistake, not a hint.
    let with_intent = json!({ "op": "set_control", "module_id": "osc", "key": "frequency",
                              "value": 1.0, "intent": "perform" });
    assert!(serde_json::from_value::<StructuralEdit>(with_intent).is_err());
    let swap = json!({ "op": "swap_module", "id": "osc", "module_type": "lfo" });
    assert!(serde_json::from_value::<StructuralEdit>(swap).is_err());
}

#[test]
fn apply_edits_request_round_trips_with_its_ticket() {
    let request = RpcRequest::new(apply(vec![add("lfo")]))
        .with_mutation(ticket("t-1", revision("s1", 2)))
        .expecting(revision("s1", 2));
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["command"], "apply_edits");
    assert_eq!(json["edits"][0]["op"], "add_module");
    assert_eq!(json["mutation"]["id"], "t-1");
    assert_eq!(serde_json::from_value::<RpcRequest>(json).unwrap(), request);
}

#[test]
fn edits_applied_response_is_compact_and_flat() {
    let report = ApplyEditsReport {
        edit_count: 3,
        added: vec!["lfo".into()],
        removed: vec![],
        rebuilt: vec![],
        controls_written: vec![WrittenControl::new("osc", "frequency")],
        controls_failed: vec![],
        connections_added: 1,
        connections_removed: 0,
        untouched: 2,
    };
    let response = RpcResponse::ok(Some("r".into()), RpcResponsePayload::EditsApplied(report))
        .with_revision(revision("s1", 5));
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["kind"], "edits_applied");
    // The committed revision is the envelope's.
    assert_eq!(json["revision"]["revision"], 5);
    assert_eq!(json["edit_count"], 3);
    assert_eq!(json["added"], json!(["lfo"]));
    assert_eq!(
        json["controls_written"],
        json!([{ "module_id": "osc", "key": "frequency" }])
    );
    assert_eq!(json["untouched"], 2);
    // Nothing failed, so the field is absent.
    assert!(json.get("controls_failed").is_none());
    assert_eq!(
        serde_json::from_value::<RpcResponse>(json).unwrap(),
        response
    );
}

#[test]
fn a_report_missing_fields_reads_them_as_defaults() {
    let report: ApplyEditsReport =
        serde_json::from_value(json!({ "edit_count": 1, "added": ["lfo"] })).unwrap();
    assert_eq!(
        report,
        ApplyEditsReport {
            edit_count: 1,
            added: vec!["lfo".into()],
            ..ApplyEditsReport::default()
        }
    );
}

#[test]
fn a_control_that_failed_at_commit_is_reported_with_its_edit() {
    let report = ApplyEditsReport {
        edit_count: 2,
        controls_failed: vec![ControlWriteFailure {
            edit_index: 1,
            module_id: "sampler".into(),
            key: "sample".into(),
            error: "file not found".into(),
        }],
        ..ApplyEditsReport::default()
    };
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["controls_failed"],
        json!([{ "edit_index": 1, "module_id": "sampler", "key": "sample",
                 "error": "file not found" }])
    );
    assert_eq!(
        serde_json::from_value::<ApplyEditsReport>(json).unwrap(),
        report
    );
}

#[test]
fn invalid_edit_error_carries_a_structured_detail() {
    let failure = EditFailure::new(
        2,
        EditOp::Connect,
        EditFailureReason::UnknownPort,
        "module 'osc' has no input port 'fn'",
    );
    let error = RpcError::invalid_edit(failure.clone());
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    assert!(error.message.contains("edit 2 (connect)"));
    assert!(error.message.contains("nothing was applied"));

    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["code"], "invalid_edit");
    assert_eq!(
        json["edit"],
        json!({ "index": 2, "op": "connect", "reason": "unknown_port",
                "message": "module 'osc' has no input port 'fn'" })
    );
    assert!(json.get("conflict").is_none());
    assert_eq!(serde_json::from_value::<RpcError>(json).unwrap(), error);

    // Batch-level refusals carry no edit detail.
    let batch = serde_json::to_value(check_edit_batch(&[]).unwrap_err()).unwrap();
    assert_eq!(batch["code"], "invalid_request");
    assert!(batch.get("edit").is_none());
}

#[test]
fn every_failure_reason_has_a_snake_case_wire_name() {
    use EditFailureReason::*;
    let names: Vec<String> = [
        InvalidName,
        UnknownModule,
        DuplicateModule,
        UnknownModuleType,
        InvalidConfig,
        UnknownPort,
        ConnectionExists,
        ConnectionNotFound,
        UnknownControl,
        InvalidControlValue,
    ]
    .into_iter()
    .map(|reason| {
        serde_json::to_value(reason)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    })
    .collect();
    assert_eq!(
        names,
        [
            "invalid_name",
            "unknown_module",
            "duplicate_module",
            "unknown_module_type",
            "invalid_config",
            "unknown_port",
            "connection_exists",
            "connection_not_found",
            "unknown_control",
            "invalid_control_value",
        ]
    );
    for op in [
        EditOp::AddModule,
        EditOp::RemoveModule,
        EditOp::Connect,
        EditOp::Disconnect,
        EditOp::SetControl,
    ] {
        assert_eq!(serde_json::to_value(op).unwrap(), json!(op.as_str()));
    }
}

#[test]
fn batch_size_is_bounded_on_both_sides() {
    let empty = check_edit_batch(&[]).unwrap_err();
    assert_eq!(empty.code, RpcErrorCode::InvalidRequest);
    assert!(check_edit_batch(&[add("a")]).is_ok());
    let full: Vec<StructuralEdit> = (0..MAX_EDITS_PER_BATCH)
        .map(|i| add(&format!("m{i}")))
        .collect();
    assert!(check_edit_batch(&full).is_ok());
    let over: Vec<StructuralEdit> = (0..=MAX_EDITS_PER_BATCH)
        .map(|i| add(&format!("m{i}")))
        .collect();
    let error = check_edit_batch(&over).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidRequest);
    assert!(error.edit.is_none());
}

#[test]
fn names_must_be_present_and_bounded() {
    let at_limit = "a".repeat(MAX_EDIT_NAME_BYTES);
    assert!(add(&at_limit).check_names(0).is_ok());

    let too_long = add(&"a".repeat(MAX_EDIT_NAME_BYTES + 1))
        .check_names(4)
        .unwrap_err();
    assert_eq!(too_long.index, 4);
    assert_eq!(too_long.op, EditOp::AddModule);
    assert_eq!(too_long.reason, EditFailureReason::InvalidName);

    let empty_port = StructuralEdit::Connect {
        from: "a".into(),
        from_port: "audio".into(),
        to: "b".into(),
        to_port: String::new(),
    }
    .check_names(1)
    .unwrap_err();
    assert_eq!(empty_port.reason, EditFailureReason::InvalidName);
    assert!(empty_port.message.contains("to_port"));

    let empty_key = StructuralEdit::SetControl {
        module_id: "osc".into(),
        key: String::new(),
        value: ControlValue::Number(1.0),
    }
    .check_names(0)
    .unwrap_err();
    assert!(empty_key.message.contains("key"));
}

#[test]
fn apply_edits_advances_the_revision_only_when_it_commits() {
    let batch = apply(vec![add("a")]);
    assert!(batch.advances_revision());
    assert!(batch.is_all_or_nothing());
    assert!(batch.advances_revision_after(true));
    assert!(!batch.advances_revision_after(false));

    // Other authoring commands advance on any attempt.
    let single = RpcCommand::RemoveModule { id: "a".into() };
    assert!(!single.is_all_or_nothing());
    assert!(single.advances_revision_after(true));
    assert!(single.advances_revision_after(false));

    // Reads never advance.
    assert!(!RpcCommand::ListPackages.advances_revision_after(true));
    assert!(!RpcCommand::ListPackages.advances_revision_after(false));
}

#[test]
fn apply_edits_requires_a_ticket_and_replays_with_it() {
    let batch = apply(vec![add("a")]);
    assert!(batch.requires_ticket());
    assert_eq!(batch.replay_policy(), ReplayPolicy::ResendWithTicket);

    let bare = RpcRequest::new(batch.clone()).check_ticket().unwrap_err();
    assert_eq!(bare.code, RpcErrorCode::InvalidRequest);
    assert!(bare.edit.is_none());
    assert!(RpcRequest::new(batch)
        .with_mutation(ticket("t", revision("s1", 0)))
        .check_ticket()
        .is_ok());

    // Other commands keep tickets optional.
    let single = RpcCommand::RemoveModule { id: "a".into() };
    assert!(!single.requires_ticket());
    assert!(RpcRequest::new(single).check_ticket().is_ok());
    assert!(RpcRequest {
        payload: RpcRequestPayload::GetSnapshot,
        ..RpcRequest::new(RpcCommand::Shutdown)
    }
    .check_ticket()
    .is_ok());
}

/// Stands in for the daemon around an all-or-nothing command: admit, run,
/// advance only on commit, record.
fn run(
    ledger: &mut MutationLedger,
    revisions: &mut RevisionTracker,
    ticket: &MutationTicket,
    command: &RpcCommand,
    response: RpcResponsePayload,
) -> RpcResponsePayload {
    match ledger.admit(ticket, command, revisions).unwrap() {
        Admission::Replay(payload) => *payload,
        Admission::Execute(pending) => {
            let committed = !matches!(response, RpcResponsePayload::Error(_));
            if command.advances_revision_after(committed) {
                revisions.advance();
            }
            ledger.record(pending, &response, revisions.current());
            response
        }
    }
}

#[test]
fn a_committed_batch_replays_as_a_compact_mutation_committed() {
    let mut ledger = MutationLedger::default();
    let mut revisions = RevisionTracker::new("s1");
    let t = ticket("b-1", revisions.current());
    let batch = apply(vec![add("a")]);
    let report = RpcResponsePayload::EditsApplied(ApplyEditsReport {
        edit_count: 1,
        added: vec!["a".into()],
        ..ApplyEditsReport::default()
    });

    assert_eq!(
        run(&mut ledger, &mut revisions, &t, &batch, report.clone()),
        report
    );
    assert_eq!(revisions.current(), revision("s1", 1));
    let retry = run(
        &mut ledger,
        &mut revisions,
        &t,
        &batch,
        RpcResponsePayload::Ack,
    );
    assert_eq!(
        retry,
        RpcResponsePayload::MutationCommitted {
            mutation_id: "b-1".into(),
            committed_at: revision("s1", 1),
        }
    );
    assert_eq!(revisions.current(), revision("s1", 1));
}

#[test]
fn a_refused_batch_replays_its_refusal_and_never_expires_fresh_tickets() {
    let mut ledger = MutationLedger::with_capacity(2);
    let mut revisions = RevisionTracker::new("s1");
    let batch = apply(vec![StructuralEdit::RemoveModule { id: "x".into() }]);
    let refusal = RpcResponsePayload::Error(RpcError::invalid_edit(EditFailure::new(
        0,
        EditOp::RemoveModule,
        EditFailureReason::UnknownModule,
        "x".repeat(MAX_RECORDED_MESSAGE_BYTES + 10),
    )));

    // Three refusals at one revision: none advances, so evicting the first
    // must not move the horizon to the current revision.
    for id in ["r-1", "r-2", "r-3"] {
        let t = ticket(id, revisions.current());
        run(&mut ledger, &mut revisions, &t, &batch, refusal.clone());
    }
    assert_eq!(revisions.current(), revision("s1", 0));

    let replayed = run(
        &mut ledger,
        &mut revisions,
        &ticket("r-3", revision("s1", 0)),
        &batch,
        RpcResponsePayload::Ack,
    );
    let RpcResponsePayload::Error(error) = replayed else {
        panic!("expected the recorded refusal");
    };
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    let edit = error.edit.expect("detail survives replay");
    assert_eq!(edit.reason, EditFailureReason::UnknownModule);
    // The recorded detail is bounded like the message.
    assert_eq!(edit.message.len(), MAX_RECORDED_MESSAGE_BYTES);

    // A fresh ticket minted at the same revision still runs.
    let fresh = ticket("r-4", revisions.current());
    assert!(matches!(
        ledger.admit(&fresh, &batch, &revisions),
        Ok(Admission::Execute(_))
    ));
}
