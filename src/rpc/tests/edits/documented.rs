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
    { "op": "connect", "from": "tremolo", "from_port": "bipolar", "to": "lead", "to_port": "amplitude_mod" },
    { "op": "set_control", "module_id": "lead", "key": "amplitude_mod_depth", "value": 0.3 }
  ]
}"#;

#[test]
fn the_documented_request_parses_through_the_envelope() {
    let request: RpcRequest = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    assert_eq!(request.mutation.as_ref().unwrap().id, "editor-7f3a-0042");
    assert!(request.check_ticket().is_ok());
    let RpcRequestPayload::Command(RpcCommand::ApplyEdits { edits, .. }) = &request.payload else {
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
            key: "amplitude_mod_depth".into(),
            value: ControlValue::Number(0.3),
        }
    );
}

#[test]
fn unknown_fields_are_refused_inside_an_edit_but_not_on_the_envelope() {
    let mut inside: serde_json::Value = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    inside["edits"][1]["to_prot"] = json!("amplitude_mod");
    assert!(serde_json::from_value::<RpcRequest>(inside).is_err());

    // Like every other command, the envelope and the command ignore fields
    // they do not know.
    let mut outside: serde_json::Value = serde_json::from_str(DOCUMENTED_REQUEST).unwrap();
    outside["dry_run"] = json!(true);
    assert!(serde_json::from_value::<RpcRequest>(outside).is_ok());
}

#[test]
fn the_documented_move_parses_and_round_trips() {
    let json =
        r#"{ "op": "move_modules", "positions": { "osc": { "x": 120, "y": 40 }, "vca": null } }"#;
    let edit: StructuralEdit = serde_json::from_str(json).unwrap();
    assert_eq!(edit.op(), EditOp::MoveModules);
    assert_eq!(EditOp::MoveModules.as_str(), "move_modules");
    let StructuralEdit::MoveModules { positions } = &edit else {
        panic!("expected move_modules, got {edit:?}");
    };
    let osc = positions["osc"].expect("osc is placed");
    assert_eq!((osc.x, osc.y), (120.0, 40.0));
    assert_eq!(positions["vca"], None);
    let again: StructuralEdit = serde_json::to_string(&edit)
        .and_then(|json| serde_json::from_str(&json))
        .unwrap();
    assert_eq!(again, edit);
    // A misspelled field is refused, as in every other edit.
    assert!(
        serde_json::from_str::<StructuralEdit>(r#"{ "op": "move_modules", "position": {} }"#)
            .is_err()
    );
}

#[test]
fn the_documented_naming_parses_and_round_trips() {
    let json = r#"{ "op": "name_modules", "names": { "tone": "Lead filter", "osc": null } }"#;
    let edit: StructuralEdit = serde_json::from_str(json).unwrap();
    assert_eq!(edit.op(), EditOp::NameModules);
    assert_eq!(EditOp::NameModules.as_str(), "name_modules");
    let StructuralEdit::NameModules { names } = &edit else {
        panic!("expected name_modules, got {edit:?}");
    };
    assert_eq!(names["tone"].as_deref(), Some("Lead filter"));
    assert_eq!(names["osc"], None);
    let again: StructuralEdit = serde_json::to_string(&edit)
        .and_then(|json| serde_json::from_str(&json))
        .unwrap();
    assert_eq!(again, edit);
    assert!(
        serde_json::from_str::<StructuralEdit>(r#"{ "op": "name_modules", "name": {} }"#).is_err()
    );
    // An add_module without a name serializes as it always has.
    let add = r#"{"op":"add_module","id":"lfo","module_type":"lfo","config":null}"#;
    let edit: StructuralEdit = serde_json::from_str(add).unwrap();
    assert_eq!(serde_json::to_string(&edit).unwrap(), add);
}
