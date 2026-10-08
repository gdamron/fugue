//! The request `docs/apply-edits.md` shows, parsed as a client sends it.

use super::*;

/// The request exactly as `docs/apply-edits.md` shows it.
const DOCUMENTED_REQUEST: &str = r#"{
  "schema_version": 1,
  "kind": "command",
  "command": "apply_edits",
  "mutation": { "id": "editor-7f3a-0042", "issued_at": { "session_id": "…", "revision": 12 } },
  "expected_revision": { "session_id": "…", "revision": 12 },
  "edits": [
    { "op": "add_module", "id": "tremolo", "module_type": "lfo", "config": { "rate": 5 } },
    { "op": "connect", "from": "tremolo", "from_port": "bipolar", "to": "lead", "to_port": "am" },
    { "op": "set_control", "module_id": "lead", "key": "am_amount", "value": 0.3 }
  ]
}"#;

#[test]
fn the_documented_request_parses_through_the_envelope() {
    let request: RpcRequest = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    assert_eq!(request.mutation.as_ref().unwrap().id, "editor-7f3a-0042");
    assert!(request.check_ticket().is_ok());
    let RpcRequestPayload::Command(RpcCommand::ApplyEdits { edits }) = &request.payload else {
        panic!("expected apply_edits, got {:?}", request.payload);
    };
    assert_eq!(edits.len(), 3);
    assert_eq!(
        edits.iter().map(StructuralEdit::op).collect::<Vec<_>>(),
        [EditOp::AddModule, EditOp::Connect, EditOp::SetControl]
    );
    assert_eq!(
        edits[2],
        StructuralEdit::SetControl {
            module_id: "lead".into(),
            key: "am_amount".into(),
            value: ControlValue::Number(0.3),
        }
    );
}

#[test]
fn unknown_fields_are_refused_inside_an_edit_but_not_on_the_envelope() {
    let mut inside: serde_json::Value = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    inside["edits"][1]["to_prot"] = json!("am");
    assert!(serde_json::from_value::<RpcRequest>(inside).is_err());

    // Like every other command, the envelope and the command ignore fields
    // they do not know.
    let mut outside: serde_json::Value = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    outside["dry_run"] = json!(true);
    assert!(serde_json::from_value::<RpcRequest>(outside).is_ok());
}
