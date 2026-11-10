//! `move_modules` changes only the document's layout, and a removed module
//! takes its position with it.

use std::collections::BTreeMap;

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{EditFailureReason, EditOp, RequestState, RpcErrorCode, MAX_MOVES_PER_EDIT};

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
    assert_eq!(moved_pump.render(200), control_pump.render(200));
    let before = observe(&moved);
    let published = publications(&moved);
    let events = Events::listen(&moved);

    let report = moved
        .apply_edits(&[moves(&[
            ("osc1", at(120.5, -40.0)),
            ("dac", at(300.0, 0.0)),
            // Unplacing a module that has no position is a move too.
            ("spare", None),
        ])])
        .expect("the move commits");

    assert_eq!(report.edit_count, 1);
    assert!(report.added.is_empty() && report.removed.is_empty() && report.rebuilt.is_empty());
    assert!(report.controls_written.is_empty());
    assert_eq!(report.untouched, 4);
    // Nothing published, nothing announced, nothing heard.
    assert_eq!(publications(&moved), published);
    assert_eq!(events.count(), 0);
    let heard = control_pump.render(300);
    assert!(heard.iter().any(|sample| sample.abs() > 0.01), "silent");
    assert_eq!(moved_pump.render(300), heard);
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
    let non_finite = [
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (f64::INFINITY, 0.0),
        (0.0, f64::NEG_INFINITY),
    ];
    let non_finite = non_finite.map(|(x, y)| moves(&[("osc1", at(x, y))]));
    for edit in std::iter::once(moves(&[])).chain(non_finite) {
        let error = running.apply_edits(&[edit]).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidEdit);
        let edit = error.edit.expect("an edit detail");
        assert_eq!(edit.reason, EditFailureReason::InvalidPosition);
    }
    assert!(placed(&running).is_empty());
}

#[test]
fn a_move_places_at_most_the_limit_and_refuses_one_more_with_nothing_changed() {
    let count = MAX_MOVES_PER_EDIT + 1;
    assert_eq!(MAX_MOVES_PER_EDIT, 1024);
    let modules: Vec<_> = (0..count)
        .map(|i| json!({ "id": format!("m{i}"), "type": "vca", "config": {} }))
        .collect();
    let document = json!({ "version": "1.0.0", "modules": modules, "connections": [] });
    let (mut running, _pump) = start(&document.to_string());
    let first = |count: usize| {
        let positions: Vec<(String, Option<Position>)> = (0..count)
            .map(|i| (format!("m{i}"), at(i as f64, -(i as f64))))
            .collect();
        StructuralEdit::MoveModules {
            positions: positions.into_iter().collect(),
        }
    };

    // Every id exists, so the only fault is the count.
    let error = running.apply_edits(&[first(count)]).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    let edit = error.edit.expect("an edit detail");
    assert_eq!(
        (edit.index, edit.op, edit.reason),
        (0, EditOp::MoveModules, EditFailureReason::InvalidPosition)
    );
    assert!(placed(&running).is_empty(), "the refusal changed nothing");

    running.apply_edits(&[first(MAX_MOVES_PER_EDIT)]).unwrap();
    let layout = placed(&running);
    assert_eq!(layout.len(), MAX_MOVES_PER_EDIT);
    assert_eq!(
        layout["m1023"],
        Position {
            x: 1023.0,
            y: -1023.0
        }
    );
    assert!(!layout.contains_key("m1024"));
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
            moves(&[("lfo1", at(1.0, 1.0))]),
            // A later move in the batch wins.
            moves(&[("lfo1", at(40.0, 80.0))]),
        ])
        .unwrap();
    assert_eq!(report.added, ["lfo1"]);
    assert_eq!(placed(&running)["lfo1"], Position { x: 40.0, y: 80.0 });
}

#[test]
fn moving_and_removing_a_module_in_one_batch_leaves_no_position() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("spare", at(1.0, 1.0))]), remove("spare")])
        .unwrap();
    assert!(placed(&running).is_empty());
}

#[test]
fn removing_and_adding_an_id_again_drops_its_position() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("spare", at(1.0, 1.0))])])
        .unwrap();
    running
        .apply_edits(&[remove("spare"), add("spare", "oscillator", json!({}))])
        .unwrap();
    assert!(placed(&running).is_empty());
}

#[test]
fn a_move_while_a_timed_edit_waits_neither_cancels_it_nor_is_lost() {
    let (mut running, pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("osc1", at(1.0, 2.0)), ("spare", at(3.0, 4.0))])])
        .unwrap();
    // Adds `lfo1` and removes `spare` 2048 samples from now.
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let batch = [add("lfo1", "lfo", json!({ "rate": 2.0 })), remove("spare")];
    let queued = running
        .apply_edits_at(&batch, &when, None)
        .expect("the edit is queued");
    assert_eq!(queued.state, RequestState::Queued, "{queued:?}");
    let published = publications(&running);
    // Moved while it waits: osc1, osc2, and spare, which the edit removes.
    running
        .apply_edits(&[moves(&[
            ("osc1", at(7.0, 8.0)),
            ("osc2", at(-1.0, -2.0)),
            ("spare", at(30.0, 40.0)),
        ])])
        .expect("the move commits");
    assert_eq!(publications(&running), published);
    assert_eq!(placed(&running)["osc1"], Position { x: 7.0, y: 8.0 });
    let waiting = running.request_status(&queued.id).expect("remembered");
    assert_eq!(waiting.state, RequestState::Queued, "{waiting:?}");
    assert!(running
        .document()
        .unwrap()
        .modules
        .iter()
        .all(|spec| spec.id != "lfo1"));

    // Past its sample: the edit lands and is adopted.
    pump.render(64);
    running.settle_requests();
    let adopted = running.request_status(&queued.id).expect("remembered");
    assert_eq!(adopted.state, RequestState::Adopted, "{adopted:?}");
    let document = running.document().unwrap();
    let ids: Vec<&str> = document
        .modules
        .iter()
        .map(|spec| spec.id.as_str())
        .collect();
    assert!(ids.contains(&"lfo1"), "the edit landed: {ids:?}");
    assert!(!ids.contains(&"spare"), "{ids:?}");
    // The moves were kept; the removed module's position went with it, and
    // the added one has none.
    let layout = &document.layout.modules;
    let placed_ids: Vec<&str> = layout.keys().map(String::as_str).collect();
    assert_eq!(placed_ids, ["osc1", "osc2"]);
    assert_eq!(layout["osc1"], Position { x: 7.0, y: 8.0 });
    assert_eq!(layout["osc2"], Position { x: -1.0, y: -2.0 });
    let saved = running.document_to_save().unwrap();
    assert_eq!(saved.layout, document.layout);
}

#[test]
fn a_timed_batch_moving_and_adding_keeps_moves_made_while_it_waits() {
    let (mut running, pump) = start(BASE);
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let batch = [
        add("lfo1", "lfo", json!({ "rate": 2.0 })),
        moves(&[("lfo1", at(1.0, 1.0)), ("osc2", at(2.0, 2.0))]),
        remove("spare"),
    ];
    running
        .apply_edits(&[moves(&[("spare", at(9.0, 9.0))])])
        .unwrap();
    running.apply_edits_at(&batch, &when, None).expect("queued");
    // While it waits, a module it does not move is moved, and one it does.
    running
        .apply_edits(&[moves(&[("osc1", at(5.0, 5.0)), ("osc2", at(6.0, 6.0))])])
        .unwrap();
    pump.render(64);
    running.live.adopt_landed();
    let layout = placed(&running);
    let ids: Vec<&str> = layout.keys().map(String::as_str).collect();
    // The batch's own moves land on top; the removed module is unplaced.
    assert_eq!(ids, ["lfo1", "osc1", "osc2"]);
    assert_eq!(layout["osc1"], Position { x: 5.0, y: 5.0 });
    assert_eq!(layout["osc2"], Position { x: 2.0, y: 2.0 });
}

#[test]
fn a_timed_batch_of_moves_alone_is_refused_as_changing_no_structure() {
    let (mut running, _pump) = start(BASE);
    let before = observe(&running);
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let error = running
        .apply_edits_at(&[moves(&[("osc1", at(1.0, 1.0))])], &when, None)
        .unwrap_err();
    assert!(error.message.contains("structural"), "{}", error.message);
    assert_eq!(observe(&running), before);
}

#[test]
fn swapping_a_module_keeps_its_position() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[moves(&[("osc1", at(3.0, 4.0))])])
        .unwrap();
    let square = json!({ "waveform": "square", "frequency": 440.0 });
    running
        .swap_module("osc1", "oscillator", &square, true)
        .unwrap();
    assert_eq!(placed(&running)["osc1"], Position { x: 3.0, y: 4.0 });
}
