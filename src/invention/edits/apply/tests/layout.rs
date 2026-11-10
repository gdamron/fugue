//! `move_modules` changes only the document's layout, and a removed module
//! takes its position with it.

use std::collections::BTreeMap;

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{EditFailureReason, EditOp, RpcErrorCode};

fn at(x: f64, y: f64) -> Option<Position> {
    Some(Position { x, y })
}

fn moves(positions: &[(&str, Option<Position>)]) -> StructuralEdit {
    let positions = positions
        .iter()
        .map(|(id, position)| (id.to_string(), *position))
        .collect();
    StructuralEdit::MoveModules { positions }
}

fn placed(running: &RunningInvention) -> BTreeMap<String, Position> {
    running.document().unwrap().layout.modules
}

#[test]
fn moving_modules_changes_the_document_alone() {
    let (mut moved, moved_pump) = start(BASE);
    let (_control, control_pump) = start(BASE);
    assert_eq!(moved_pump.render(5), control_pump.render(5));
    let before = observe(&moved);
    let published = publications(&moved);
    let events = Events::listen(&moved);

    let report = moved
        .apply_edits(&[moves(&[
            ("osc1", at(120.5, -40.0)),
            ("dac", at(300.0, 0.0)),
        ])])
        .expect("the move commits");

    assert_eq!(report.edit_count, 1);
    assert!(report.added.is_empty() && report.removed.is_empty() && report.rebuilt.is_empty());
    assert!(report.controls_written.is_empty());
    assert_eq!(report.untouched, 4);
    // Nothing published, nothing announced, nothing heard.
    assert_eq!(publications(&moved), published);
    assert_eq!(events.count(), 0);
    assert_eq!(moved_pump.render(5), control_pump.render(5));
    let after = observe(&moved);
    assert_eq!(after.modules, before.modules);
    assert_eq!(after.connections, before.connections);
    assert_eq!(after.generation, before.generation);
    // Only the layout changed in the document.
    let mut document = after.document.unwrap();
    let layout = std::mem::take(&mut document.layout);
    assert_eq!(Some(document), before.document);
    let osc1 = layout.modules["osc1"];
    assert_eq!((osc1.x, osc1.y), (120.5, -40.0));
    assert_eq!(layout.modules.len(), 2);
}

#[test]
fn moving_again_to_the_same_place_leaves_the_same_document() {
    let (mut running, _pump) = start(BASE);
    let edit = moves(&[("osc2", at(10.0, 20.0))]);
    running.apply_edits(std::slice::from_ref(&edit)).unwrap();
    let once = running.document();
    running.apply_edits(&[edit]).unwrap();
    assert_eq!(running.document(), once);
}

#[test]
fn a_null_position_unplaces_a_module() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("osc1", at(1.0, 2.0)), ("osc2", at(3.0, 4.0))])])
        .unwrap();
    running.apply_edits(&[moves(&[("osc1", None)])]).unwrap();
    let ids: Vec<String> = placed(&running).into_keys().collect();
    assert_eq!(ids, ["osc2"]);
}

#[test]
fn moving_an_unknown_module_is_refused_with_nothing_changed() {
    let (mut running, _pump) = start(BASE);
    let before = observe(&running);
    let error = running
        .apply_edits(&[moves(&[("osc1", at(1.0, 2.0)), ("ghost", at(0.0, 0.0))])])
        .unwrap_err();
    let edit = error.edit.expect("an edit detail");
    assert_eq!(
        (edit.index, edit.op, edit.reason),
        (0, EditOp::MoveModules, EditFailureReason::UnknownModule)
    );
    assert!(edit.message.contains("'ghost'"), "{}", edit.message);
    assert_eq!(observe(&running), before);
}

#[test]
fn a_module_removed_earlier_in_the_batch_cannot_be_moved() {
    let (mut running, _pump) = start(BASE);
    let error = running
        .apply_edits(&[remove("spare"), moves(&[("spare", at(0.0, 0.0))])])
        .unwrap_err();
    let edit = error.edit.expect("an edit detail");
    assert_eq!(
        (edit.index, edit.reason),
        (1, EditFailureReason::UnknownModule)
    );
}

#[test]
fn a_move_with_no_position_or_a_non_finite_one_is_refused() {
    let (mut running, _pump) = start(BASE);
    for edit in [moves(&[]), moves(&[("osc1", at(f64::NAN, 0.0))])] {
        let error = running.apply_edits(&[edit]).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidEdit);
        let edit = error.edit.expect("an edit detail");
        assert_eq!(edit.reason, EditFailureReason::InvalidPosition);
    }
    assert!(placed(&running).is_empty());
}

#[test]
fn removing_a_module_drops_its_position() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("spare", at(1.0, 1.0)), ("osc2", at(2.0, 2.0))])])
        .unwrap();
    // In a batch, alongside a move.
    running
        .apply_edits(&[moves(&[("osc1", at(5.0, 5.0))]), remove("spare")])
        .unwrap();
    let ids: Vec<String> = placed(&running).into_keys().collect();
    assert_eq!(ids, ["osc1", "osc2"]);
    // And on its own.
    running.remove_module("osc2").unwrap();
    let ids: Vec<String> = placed(&running).into_keys().collect();
    assert_eq!(ids, ["osc1"]);
}

#[test]
fn a_module_added_and_placed_in_one_batch_keeps_its_place() {
    let (mut running, _pump) = start(BASE);
    let report = running
        .apply_edits(&[
            add("lfo1", "lfo", json!({ "rate": 2.0 })),
            moves(&[("lfo1", at(40.0, 80.0))]),
        ])
        .unwrap();
    assert_eq!(report.added, ["lfo1"]);
    assert_eq!(placed(&running)["lfo1"], Position { x: 40.0, y: 80.0 });
}
