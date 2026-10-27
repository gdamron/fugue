//! The pending listing on the wire: compact unless asked for in full, and
//! a reply that omits `more` when nothing is left out.

use super::*;
use crate::ControlValue;

#[test]
fn a_bare_listing_is_compact_and_changes_nothing() {
    let bare: RpcCommand =
        serde_json::from_value(serde_json::json!({ "command": "list_pending_requests" })).unwrap();
    assert_eq!(
        bare,
        RpcCommand::ListPendingRequests {
            source: None,
            detail: PendingDetail::Compact,
        }
    );
    assert!(!bare.advances_revision() && !bare.replies_with_snapshot());

    let full = RpcCommand::ListPendingRequests {
        source: Some(PendingSource::Agent),
        detail: PendingDetail::Full,
    };
    let json = serde_json::to_value(&full).unwrap();
    assert_eq!(json["source"], "agent");
    assert_eq!(json["detail"], "full");
    assert_eq!(serde_json::from_value::<RpcCommand>(json).unwrap(), full);
}

#[test]
fn a_listing_says_how_many_more_only_when_some_are_left_out() {
    let item = PendingRequest {
        request: "7".into(),
        module_id: "osc".into(),
        key: "frequency".into(),
        requested: ControlValue::Number(330.0),
        source: PendingSource::User,
        waiting: PendingWait::Condition,
        landing: None,
    };
    let all = PendingRequests {
        total: 1,
        by_source: PendingCounts {
            user: 1,
            ..PendingCounts::default()
        },
        items: vec![item],
        more: 0,
    };
    let json = serde_json::to_value(RpcResponsePayload::PendingRequests(all.clone())).unwrap();
    assert!(json.get("more").is_none(), "{json}");
    assert_eq!(json["items"][0]["waiting"], "condition");
    let decoded: RpcResponsePayload = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, RpcResponsePayload::PendingRequests(all.clone()));

    let cut = PendingRequests {
        total: 9,
        more: 8,
        ..all
    };
    let json = serde_json::to_value(&cut).unwrap();
    assert_eq!(json["more"], 8);
}
