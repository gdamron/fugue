use super::*;
use crate::ControlValue;

fn ticket(id: &str, issued_at: RuntimeRevision) -> MutationTicket {
    MutationTicket {
        id: id.to_string(),
        issued_at,
    }
}

fn reload() -> RpcCommand {
    RpcCommand::ReloadInvention {
        invention: Box::new(test_invention()),
        source_path: None,
        frozen: false,
    }
}

fn add(id: &str) -> RpcCommand {
    RpcCommand::AddModule {
        id: id.to_string(),
        module_type: "oscillator".to_string(),
        config: serde_json::Value::Null,
    }
}

fn perform() -> RpcCommand {
    RpcCommand::SetControl {
        module_id: "osc".to_string(),
        key: "frequency".to_string(),
        value: ControlValue::Number(220.0),
        intent: ControlWriteIntent::Perform,
    }
}

/// Stands in for the daemon: admit, run (advancing the revision as every
/// attempted authoring command does), record.
fn run(
    ledger: &mut MutationLedger,
    revisions: &mut RevisionTracker,
    ticket: &MutationTicket,
    command: &RpcCommand,
    response: RpcResponsePayload,
) -> Result<RpcResponsePayload, RpcError> {
    match ledger.admit(ticket, command, revisions)? {
        Admission::Replay(payload) => Ok(*payload),
        Admission::Execute(pending) => {
            revisions.advance();
            ledger.record(pending, &response, revisions.current());
            Ok(response)
        }
    }
}

fn replayed(admission: Admission) -> RpcResponsePayload {
    match admission {
        Admission::Replay(payload) => *payload,
        Admission::Execute(_) => panic!("expected a replay, the command would run"),
    }
}

fn error_code(result: Result<RpcResponsePayload, RpcError>) -> RpcErrorCode {
    result.expect_err("expected a refusal").code
}

#[test]
fn a_retry_of_a_committed_command_replays_without_running_again() {
    let mut ledger = MutationLedger::default();
    let mut revisions = RevisionTracker::new("s1");
    let t = ticket("a-1", revisions.current());

    let first = run(
        &mut ledger,
        &mut revisions,
        &t,
        &reload(),
        RpcResponsePayload::Ack,
    );
    assert_eq!(first.unwrap(), RpcResponsePayload::Ack);
    assert_eq!(revisions.current(), revision("s1", 1));

    // The response was lost; the client resends with the same ticket.
    let retry = replayed(ledger.admit(&t, &reload(), &revisions).unwrap());
    match retry {
        RpcResponsePayload::MutationCommitted {
            mutation_id,
            committed_at: committed,
        } => {
            assert_eq!(mutation_id, "a-1");
            assert_eq!(committed, revision("s1", 1));
        }
        other => panic!("expected a committed replay, got {other:?}"),
    }
    assert_eq!(ledger.len(), 1);
}

#[test]
fn a_retry_of_a_rejected_command_replays_the_rejection() {
    let mut ledger = MutationLedger::default();
    let mut revisions = RevisionTracker::new("s1");
    let t = ticket("a-1", revisions.current());
    let refusal = RpcResponsePayload::Error(RpcError::new(
        RpcErrorCode::UnknownModuleType,
        "no such module type",
    ));

    run(&mut ledger, &mut revisions, &t, &add("x"), refusal.clone()).unwrap();
    let retry = replayed(ledger.admit(&t, &add("x"), &revisions).unwrap());
    assert_eq!(retry, refusal);
}

#[test]
fn a_reused_id_with_a_different_command_is_refused() {
    let mut ledger = MutationLedger::default();
    let mut revisions = RevisionTracker::new("s1");
    let t = ticket("a-1", revisions.current());
    run(
        &mut ledger,
        &mut revisions,
        &t,
        &add("x"),
        RpcResponsePayload::Ack,
    )
    .unwrap();

    let reused = ledger.admit(&t, &add("y"), &revisions);
    assert_eq!(
        reused.expect_err("different content").code,
        RpcErrorCode::InvalidRequest
    );
}

#[test]
fn a_ticket_from_a_replaced_daemon_is_fenced_off() {
    let ledger = MutationLedger::default();
    let replacement = RevisionTracker::new("s2");
    let t = ticket("a-1", revision("s1", 0));

    let error = ledger.admit(&t, &reload(), &replacement).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::RevisionConflict);
    let conflict = error.conflict.expect("structured conflict");
    assert_eq!(conflict.reason, ConflictReason::SessionReplaced);
    assert_eq!(conflict.current, revision("s2", 0));
}

#[test]
fn malformed_tickets_are_refused() {
    let ledger = MutationLedger::default();
    let revisions = RevisionTracker::new("s1");
    let refuse = |t: &MutationTicket, command: &RpcCommand| {
        ledger.admit(t, command, &revisions).unwrap_err().code
    };

    assert_eq!(
        refuse(&ticket("", revision("s1", 0)), &reload()),
        RpcErrorCode::InvalidRequest
    );
    let long = "x".repeat(MAX_MUTATION_ID_BYTES + 1);
    assert_eq!(
        refuse(&ticket(&long, revision("s1", 0)), &reload()),
        RpcErrorCode::InvalidRequest
    );
    // Issued at a revision the daemon has not reached.
    assert_eq!(
        refuse(&ticket("a-1", revision("s1", 5)), &reload()),
        RpcErrorCode::InvalidRequest
    );
    // Performance writes and reads are not ticketed.
    assert_eq!(
        refuse(&ticket("a-1", revision("s1", 0)), &perform()),
        RpcErrorCode::InvalidRequest
    );
    assert_eq!(
        refuse(&ticket("a-1", revision("s1", 0)), &RpcCommand::ListPackages),
        RpcErrorCode::InvalidRequest
    );
}

#[test]
fn an_evicted_outcome_expires_instead_of_running_twice() {
    let mut ledger = MutationLedger::with_capacity(2);
    let mut revisions = RevisionTracker::new("s1");
    let first = ticket("a-1", revisions.current());
    run(
        &mut ledger,
        &mut revisions,
        &first,
        &add("a"),
        RpcResponsePayload::Ack,
    )
    .unwrap();
    for id in ["a-2", "a-3"] {
        let t = ticket(id, revisions.current());
        run(
            &mut ledger,
            &mut revisions,
            &t,
            &add(id),
            RpcResponsePayload::Ack,
        )
        .unwrap();
    }
    assert_eq!(ledger.len(), 2, "the ledger stays bounded");

    // "a-1" was forgotten; its retry must not run again.
    assert_eq!(
        error_code(run(
            &mut ledger,
            &mut revisions,
            &first,
            &add("a"),
            RpcResponsePayload::Ack
        )),
        RpcErrorCode::MutationExpired
    );
    assert_eq!(revisions.current(), revision("s1", 3), "nothing ran");

    // A ticket minted now is past the horizon, so it still runs.
    let fresh = ticket("a-4", revisions.current());
    assert!(matches!(
        ledger.admit(&fresh, &add("d"), &revisions).unwrap(),
        Admission::Execute(_)
    ));
}

#[test]
fn a_never_seen_ticket_after_the_horizon_runs() {
    // The original request was lost before the daemon read it; enough later
    // edits ran to evict older outcomes, but not any issued after this ticket.
    let mut ledger = MutationLedger::with_capacity(1);
    let mut revisions = RevisionTracker::new("s1");
    let old = ticket("other-1", revisions.current());
    run(
        &mut ledger,
        &mut revisions,
        &old,
        &add("a"),
        RpcResponsePayload::Ack,
    )
    .unwrap();
    let lost = ticket("mine-1", revisions.current());
    let later = ticket("other-2", revisions.current());
    run(
        &mut ledger,
        &mut revisions,
        &later,
        &add("b"),
        RpcResponsePayload::Ack,
    )
    .unwrap();

    // Horizon is revision 0 ("other-1" ran there); "mine-1" was issued at 1.
    assert!(matches!(
        ledger.admit(&lost, &reload(), &revisions).unwrap(),
        Admission::Execute(_)
    ));
}

#[test]
fn recorded_rejections_are_bounded() {
    let mut ledger = MutationLedger::default();
    let mut revisions = RevisionTracker::new("s1");
    let t = ticket("a-1", revisions.current());
    let long = RpcResponsePayload::Error(RpcError::new(
        RpcErrorCode::ModuleBuildFailed,
        "é".repeat(MAX_RECORDED_MESSAGE_BYTES),
    ));
    run(&mut ledger, &mut revisions, &t, &add("x"), long).unwrap();

    match replayed(ledger.admit(&t, &add("x"), &revisions).unwrap()) {
        RpcResponsePayload::Error(error) => {
            assert_eq!(error.code, RpcErrorCode::ModuleBuildFailed);
            assert!(error.message.len() <= MAX_RECORDED_MESSAGE_BYTES);
        }
        other => panic!("expected a rejected replay, got {other:?}"),
    }
}

#[test]
fn replay_policy_separates_reads_authoring_and_gestures() {
    use ReplayPolicy::*;
    let command = |c: RpcCommand| RpcRequestPayload::Command(c).replay_policy();

    assert_eq!(RpcRequestPayload::GetSnapshot.replay_policy(), Resend);
    assert_eq!(RpcRequestPayload::Hello.replay_policy(), Resend);
    assert_eq!(command(RpcCommand::ListPackages), Resend);
    assert_eq!(
        command(RpcCommand::SaveInvention {
            path: "/tmp/x.json".into()
        }),
        Resend
    );

    assert_eq!(command(reload()), ResendWithTicket);
    assert_eq!(command(add("x")), ResendWithTicket);
    assert_eq!(
        command(RpcCommand::SetControl {
            module_id: "osc".into(),
            key: "frequency".into(),
            value: ControlValue::Number(1.0),
            intent: ControlWriteIntent::Author,
        }),
        ResendWithTicket
    );

    assert_eq!(command(perform()), Never);
    assert_eq!(
        command(RpcCommand::SetControls {
            writes: vec![ControlWrite::performed(
                "osc",
                "gate",
                ControlValue::Bool(true)
            )],
        }),
        Never
    );
    assert_eq!(command(RpcCommand::Shutdown), Never);
    assert_eq!(
        RpcRequestPayload::Subscribe { topics: Vec::new() }.replay_policy(),
        Never
    );
}

#[test]
fn tickets_travel_on_the_wire_only_when_present() {
    let plain = serde_json::to_value(RpcRequest::new(reload())).unwrap();
    assert!(plain.get("mutation").is_none());

    let request = RpcRequest::new(reload()).with_mutation(ticket("a-1", revision("s1", 3)));
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["mutation"]["id"], "a-1");
    assert_eq!(json["mutation"]["issued_at"]["revision"], 3);
    let parsed: RpcRequest = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, request);
}

#[test]
fn committed_replays_are_compact_on_the_wire() {
    let response = RpcResponse::ok(
        Some("7".into()),
        RpcResponsePayload::MutationCommitted {
            mutation_id: "a-1".into(),
            committed_at: revision("s1", 4),
        },
    );
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["kind"], "mutation_committed");
    assert_eq!(json["mutation_id"], "a-1");
    assert_eq!(json["committed_at"]["revision"], 4);
    let parsed: RpcResponse = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, response);
}
