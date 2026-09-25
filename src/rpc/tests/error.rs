use super::*;
use crate::GraphCommandError;

#[test]
fn graph_errors_map_to_rpc_errors() {
    let error = RpcError::from(GraphCommandError::UnknownModule("osc".to_string()));
    assert_eq!(error.code, RpcErrorCode::UnknownModule);
    assert!(error.message.contains("osc"));
}

#[test]
fn package_install_placeholder_is_structured_unsupported_error() {
    let error = RpcError::unsupported("package installation is not implemented yet");
    assert_eq!(error.code, RpcErrorCode::Unsupported);
    assert!(error.message.contains("not implemented"));
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
