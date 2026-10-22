//! Timed structural edits on the wire: a command without `when` reads and
//! writes exactly as before, an agent's timed command parses, and its
//! status says what became of it.

use super::*;
use crate::LoggedBeat;

fn command(json: serde_json::Value) -> Result<RpcCommand, serde_json::Error> {
    serde_json::from_value(json)
}

#[test]
fn an_untimed_command_reads_and_writes_as_before() {
    let swap = command(serde_json::json!({
        "command": "swap_module", "id": "fx", "module_type": "reverb",
        "config": { "mix": 0.5 }, "preserve_connections": true
    }))
    .unwrap();
    let RpcCommand::SwapModule { timing, .. } = &swap else {
        panic!("{swap:?}");
    };
    assert_eq!(timing, &EditTiming::default());
    assert_eq!(
        serde_json::to_string(&swap).unwrap(),
        r#"{"command":"swap_module","id":"fx","module_type":"reverb","config":{"mix":0.5},"preserve_connections":true}"#
    );
    assert!(swap.advances_revision() && swap.replies_with_snapshot());
}

#[test]
fn a_timed_command_queues_without_advancing_the_revision() {
    let swap = command(serde_json::json!({
        "command": "swap_module", "id": "fx", "module_type": "reverb",
        "when": { "kind": "next_bar" }, "ttl_ms": 4000
    }))
    .unwrap();
    let RpcCommand::SwapModule { timing, .. } = &swap else {
        panic!("{swap:?}");
    };
    let next_bar = EditWhen::NextBar {
        beats_per_bar: 4.0,
        clock: None,
    };
    assert_eq!(timing.when.as_ref(), Some(&next_bar));
    assert_eq!(timing.ttl_ms, Some(4000));
    assert!(!swap.advances_revision() && !swap.replies_with_snapshot());
    assert!(swap.is_timed());
    assert_eq!(swap.replay_policy(), ReplayPolicy::ResendWithTicket);

    let apply = command(serde_json::json!({
        "command": "apply_edits", "edits": [],
        "when": { "kind": "after_beats", "beats": 2, "clock": "clock" }
    }))
    .unwrap();
    assert!(!apply.advances_revision_after(true));
    assert!(
        apply.requires_ticket(),
        "an edit batch is still all or nothing"
    );
}

#[test]
fn a_misspelt_time_is_refused_rather_than_applied_at_once() {
    let misspelt = command(serde_json::json!({
        "command": "swap_module", "id": "fx", "module_type": "reverb",
        "when": { "kind": "next_beat", "evry": 4 }
    }));
    assert!(misspelt.is_err());
}

#[test]
fn a_status_reads_back_as_written() {
    let status = RequestStatus {
        id: "2-5".to_string(),
        state: RequestState::Adopted,
        landing: RequestLanding {
            sample: Some(96_000),
            due: None,
            beat: Some(LoggedBeat::Grid {
                clock: "clock".to_string(),
                every: 4.0,
                offset: 0.0,
                repeat: false,
            }),
        },
        reason: None,
        built: vec!["fx".to_string()],
        removed: Vec::new(),
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["state"], "adopted");
    assert_eq!(json["landing"]["sample"], 96_000);
    assert!(json.get("reason").is_none() && json.get("removed").is_none());
    assert_eq!(
        serde_json::from_value::<RequestStatus>(json).unwrap(),
        status
    );
    assert!(status.state.is_final() && !RequestState::Landed.is_final());

    let reply = RpcResponsePayload::Request(status.clone());
    let event = RpcEventPayload::RequestChanged(status);
    for json in [
        serde_json::to_value(reply).unwrap(),
        serde_json::to_value(event).unwrap(),
    ] {
        assert_eq!(json["id"], "2-5");
    }
}

#[test]
fn a_status_reply_and_a_get_request_keep_their_envelopes_request_id() {
    let get = RpcRequest::new(RpcCommand::GetRequest {
        id: "2-5".to_string(),
    });
    let get = RpcRequest {
        request_id: Some("r1".to_string()),
        ..get
    };
    let json = serde_json::to_string(&get).unwrap();
    assert_eq!(serde_json::from_str::<RpcRequest>(&json).unwrap(), get);

    let status = RequestStatus {
        id: "2-5".to_string(),
        state: RequestState::Queued,
        landing: RequestLanding::default(),
        reason: None,
        built: Vec::new(),
        removed: Vec::new(),
    };
    let reply = RpcResponse::ok(Some("r1".to_string()), RpcResponsePayload::Request(status));
    let json = serde_json::to_string(&reply).unwrap();
    assert_eq!(serde_json::from_str::<RpcResponse>(&json).unwrap(), reply);
}

/// A timed edit batch carries a ticket like any edit batch. A retry is
/// answered with the request the first attempt queued, never queues a
/// second, and evicting it marks the horizon: it may still change state.
#[test]
fn a_retried_timed_edit_replays_the_request_it_queued() {
    let apply = command(serde_json::json!({
        "command": "apply_edits", "edits": [],
        "when": { "kind": "at_sample", "sample": 0 }
    }))
    .unwrap();
    assert!(RpcRequest::new(apply.clone()).check_ticket().is_err());

    let mut ledger = MutationLedger::with_capacity(1);
    let revisions = RevisionTracker::new("s1");
    let ticket = MutationTicket {
        id: "t-1".to_string(),
        issued_at: revisions.current(),
    };
    let Admission::Execute(pending) = ledger.admit(&ticket, &apply, &revisions).unwrap() else {
        panic!("a new ticket runs");
    };
    let queued = RpcResponsePayload::Request(RequestStatus {
        id: "1-3".to_string(),
        state: RequestState::Queued,
        landing: RequestLanding::default(),
        reason: None,
        built: Vec::new(),
        removed: Vec::new(),
    });
    ledger.record(pending, &queued, revisions.current());
    match ledger.admit(&ticket, &apply, &revisions).unwrap() {
        Admission::Replay(payload) => assert_eq!(*payload, queued),
        Admission::Execute(_) => panic!("a retry would queue a second request"),
    }

    // Evicted by the next ticketed edit, its outcome is unknown.
    let other = MutationTicket {
        id: "t-2".to_string(),
        issued_at: revisions.current(),
    };
    let Admission::Execute(pending) = ledger.admit(&other, &apply, &revisions).unwrap() else {
        panic!("a new ticket runs");
    };
    ledger.record(pending, &queued, revisions.current());
    let expired = ledger.admit(&ticket, &apply, &revisions).unwrap_err();
    assert_eq!(expired.code, RpcErrorCode::MutationExpired);
}

#[test]
fn a_queued_edit_never_expires_the_tickets_minted_at_the_revision_still_current() {
    let apply = command(serde_json::json!({
        "command": "apply_edits", "edits": [],
        "when": { "kind": "at_sample", "sample": 0 }
    }))
    .unwrap();
    let mut ledger = MutationLedger::with_capacity(2);
    let mut revisions = RevisionTracker::new("s1");
    let ticket = |id: &str, revisions: &RevisionTracker| MutationTicket {
        id: id.to_string(),
        issued_at: revisions.current(),
    };
    let run = |ledger: &mut MutationLedger, id: &str, revisions: &RevisionTracker, reply| {
        let Admission::Execute(pending) = ledger
            .admit(&ticket(id, revisions), &apply, revisions)
            .unwrap()
        else {
            panic!("{id} is new, so it runs");
        };
        ledger.record(pending, reply, revisions.current());
    };
    let queued = RpcResponsePayload::Request(RequestStatus {
        id: "s1.1.1".to_string(),
        state: RequestState::Queued,
        landing: RequestLanding::default(),
        reason: None,
        built: Vec::new(),
        removed: Vec::new(),
    });
    let refused = RpcResponsePayload::Error(RpcError::new(RpcErrorCode::ControlError, "no"));

    // The queued edit expires unseen; refusals at the same revision follow
    // it past the ledger's capacity. The refusals go, and it stays.
    run(&mut ledger, "queued", &revisions, &queued);
    for id in ["r-1", "r-2", "r-3"] {
        run(&mut ledger, id, &revisions, &refused);
    }
    let retry = ledger.admit(&ticket("queued", &revisions), &apply, &revisions);
    assert!(matches!(retry, Ok(Admission::Replay(_))), "still answered");
    run(&mut ledger, "fresh", &revisions, &refused);

    // Once the revision moves on, it is evicted in turn, and only tickets
    // minted at or before its revision expire.
    let before = revisions.current();
    revisions.advance();
    run(&mut ledger, "r-4", &revisions, &refused);
    run(&mut ledger, "r-5", &revisions, &refused);
    let stale = MutationTicket {
        id: "queued".to_string(),
        issued_at: before,
    };
    let expired = ledger.admit(&stale, &apply, &revisions).unwrap_err();
    assert_eq!(expired.code, RpcErrorCode::MutationExpired);
    run(&mut ledger, "fresh-2", &revisions, &refused);
}

#[test]
fn timed_edits_that_expire_at_one_revision_never_expire_its_tickets() {
    let apply = command(serde_json::json!({
        "command": "apply_edits", "edits": [],
        "when": { "kind": "at_sample", "sample": 0 }
    }))
    .unwrap();
    let mut ledger = MutationLedger::with_capacity(2);
    let revisions = RevisionTracker::new("s1");
    let ticket = |id: &str| MutationTicket {
        id: id.to_string(),
        issued_at: revisions.current(),
    };
    let queued = |id: &str| {
        RpcResponsePayload::Request(RequestStatus {
            id: id.to_string(),
            state: RequestState::Queued,
            landing: RequestLanding::default(),
            reason: None,
            built: Vec::new(),
            removed: Vec::new(),
        })
    };

    // More timed edits than the ledger holds queue at one revision, and each
    // expires before the next is sent; queuing and expiring advance nothing.
    for n in 1..=5 {
        let id = format!("t-{n}");
        let Admission::Execute(pending) = ledger.admit(&ticket(&id), &apply, &revisions).unwrap()
        else {
            panic!("{id} is new, so it runs");
        };
        let request = format!("s1.1.{n}");
        ledger.record(pending, &queued(&request), revisions.current());
        ledger.request_unchanged(&request);
    }

    // A fresh ticket minted at that revision still runs, and the latest
    // edits still replay.
    let fresh = ledger.admit(&ticket("fresh"), &apply, &revisions);
    assert!(matches!(fresh, Ok(Admission::Execute(_))), "{fresh:?}");
    let retry = ledger.admit(&ticket("t-5"), &apply, &revisions);
    assert!(matches!(retry, Ok(Admission::Replay(_))), "still answered");
}
