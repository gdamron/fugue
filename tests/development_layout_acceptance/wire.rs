//! The edit as an agent sends it: `move_modules` with a `development` path,
//! and the refusal reasons an adapter acts on (a wire contract).

use crate::*;
use fugue::rpc::{EditFailureReason, EditOp, StructuralEdit};

#[test]
fn a_move_inside_parses_from_the_wire_and_without_a_path_places_the_piece() {
    let edit: StructuralEdit = serde_json::from_value(json!({
        "op": "move_modules",
        "development": ["voice", "env"],
        "positions": { "env": { "x": 120, "y": 40 }, "vca": null },
    }))
    .unwrap();
    assert_eq!(edit.op(), EditOp::MoveModules);
    assert_eq!(
        edit,
        inside(
            &["voice", "env"],
            &[("env", Some((120.0, 40.0))), ("vca", None)]
        )
    );

    let edit: StructuralEdit = serde_json::from_value(json!({
        "op": "move_modules", "positions": { "osc": { "x": 1, "y": 2 } },
    }))
    .unwrap();
    assert_eq!(edit, top_level(&[("osc", Some((1.0, 2.0)))]));
}

#[test]
fn the_new_refusal_reasons_have_their_wire_names() {
    for (reason, name) in [
        (EditFailureReason::UnknownDevelopment, "unknown_development"),
        (
            EditFailureReason::ReadOnlyDevelopment,
            "read_only_development",
        ),
        (
            EditFailureReason::DevelopmentNotWritten,
            "development_not_written",
        ),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(name));
    }
}
