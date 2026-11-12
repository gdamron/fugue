//! `name_modules` changes only the document's names, as `move_modules`
//! changes only its layout, and a removed module takes its name with it.

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{EditFailureReason, EditOp, RequestState, RpcErrorCode, MAX_NAMES_PER_EDIT};

fn names(names: &[(&str, Option<&str>)]) -> StructuralEdit {
    let names = names
        .iter()
        .map(|(id, name)| (id.to_string(), name.map(str::to_string)))
        .collect();
    StructuralEdit::NameModules { names }
}

fn name_of(running: &RunningInvention, id: &str) -> Option<String> {
    let document = running.document().unwrap();
    let spec = document.modules.into_iter().find(|spec| spec.id == id);
    spec.and_then(|spec| spec.name)
}

#[test]
fn naming_modules_changes_the_document_alone_and_is_never_heard() {
    let (mut named, named_pump) = start(BASE);
    let (_control, control_pump) = start(BASE);
    assert_eq!(
        named_pump.render_stereo(200),
        control_pump.render_stereo(200)
    );
    let before = observe(&named);
    let published = publications(&named);
    let events = Events::listen(&named);

    let report = named
        .apply_edits(&[names(&[("osc1", Some("Lead")), ("spare", None)])])
        .expect("the names commit");

    assert_eq!(report.edit_count, 1);
    assert!(report.added.is_empty() && report.removed.is_empty() && report.rebuilt.is_empty());
    assert!(report.controls_written.is_empty());
    assert_eq!(report.untouched, 4);
    assert_eq!(publications(&named), published);
    assert_eq!(events.count(), 0);
    let heard = control_pump.render_stereo(300);
    assert!(heard.0.iter().any(|sample| sample.abs() > 0.01), "silent");
    assert_eq!(named_pump.render_stereo(300), heard);
    let after = observe(&named);
    assert_eq!(after.modules, before.modules);
    assert_eq!(after.generation, before.generation);
    // Only osc1's name changed in the document.
    let mut document = after.document.unwrap();
    assert_eq!(document.modules[0].name.take().as_deref(), Some("Lead"));
    assert_eq!(Some(document), before.document);
}

#[test]
fn a_null_name_clears_it_and_naming_again_is_idempotent() {
    let (mut running, _pump) = start(BASE);
    let edit = names(&[("osc1", Some("Lead")), ("osc2", Some("Pad"))]);
    running.apply_edits(std::slice::from_ref(&edit)).unwrap();
    let once = running.document();
    running.apply_edits(&[edit]).unwrap();
    assert_eq!(running.document(), once);
    running.apply_edits(&[names(&[("osc1", None)])]).unwrap();
    assert_eq!(name_of(&running, "osc1"), None);
    assert_eq!(name_of(&running, "osc2").as_deref(), Some("Pad"));
}

#[test]
fn names_and_moves_apply_together_as_one_document_change() {
    let (mut running, _pump) = start(BASE);
    let published = publications(&running);
    let positions = [("osc1".to_string(), Some(Position { x: 1.0, y: 2.0 }))];
    running
        .apply_edits(&[
            StructuralEdit::MoveModules {
                positions: positions.into_iter().collect(),
            },
            names(&[("osc1", Some("Lead"))]),
        ])
        .unwrap();
    assert_eq!(publications(&running), published);
    let document = running.document().unwrap();
    assert_eq!(document.layout.modules.len(), 1);
    assert_eq!(name_of(&running, "osc1").as_deref(), Some("Lead"));
}

#[test]
fn naming_an_unknown_module_is_refused_with_nothing_changed() {
    let (mut running, _pump) = start(BASE);
    let before = observe(&running);
    let error = running
        .apply_edits(&[names(&[("osc1", Some("Lead")), ("ghost", Some("Boo"))])])
        .unwrap_err();
    let edit = error.edit.expect("an edit detail");
    assert_eq!(
        (edit.index, edit.op, edit.reason),
        (0, EditOp::NameModules, EditFailureReason::UnknownModule)
    );
    assert!(edit.message.contains("'ghost'"), "{}", edit.message);
    assert_eq!(observe(&running), before);
}

#[test]
fn a_name_that_cannot_name_a_module_is_refused_with_nothing_changed() {
    let (mut running, _pump) = start(BASE);
    let before = observe(&running);
    let long = "x".repeat(crate::invention::MAX_MODULE_NAME_BYTES + 1);
    let refused = [
        names(&[]),
        names(&[("osc1", Some(""))]),
        names(&[("osc1", Some("  "))]),
        names(&[("osc1", Some("two\nlines"))]),
        names(&[("osc1", Some(&long))]),
    ];
    for edit in refused {
        let error = running.apply_edits(&[edit]).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidEdit);
        let edit = error.edit.expect("an edit detail");
        assert_eq!(
            (edit.op, edit.reason),
            (EditOp::NameModules, EditFailureReason::InvalidName)
        );
        assert_eq!(observe(&running), before, "the refusal changed nothing");
    }
    let too_many = StructuralEdit::NameModules {
        names: (0..=MAX_NAMES_PER_EDIT)
            .map(|i| (format!("m{i}"), Some("x".to_string())))
            .collect(),
    };
    let error = running.apply_edits(&[too_many]).unwrap_err();
    assert_eq!(
        error.edit.map(|edit| edit.reason),
        Some(EditFailureReason::InvalidName)
    );
}

#[test]
fn a_module_added_with_a_name_keeps_it_and_a_bad_one_is_refused() {
    let (mut running, _pump) = start(BASE);
    let named = |name: &str| StructuralEdit::AddModule {
        id: "lfo1".into(),
        module_type: "lfo".into(),
        config: json!({ "rate": 2.0 }),
        name: Some(name.into()),
    };
    let error = running.apply_edits(&[named(" ")]).unwrap_err();
    let edit = error.edit.expect("an edit detail");
    assert_eq!(
        (edit.op, edit.reason),
        (EditOp::AddModule, EditFailureReason::InvalidName)
    );
    let report = running.apply_edits(&[named("Wobble")]).unwrap();
    assert_eq!(report.added, ["lfo1"]);
    assert_eq!(name_of(&running, "lfo1").as_deref(), Some("Wobble"));
}

#[test]
fn removing_a_module_drops_its_name_and_adding_the_id_again_starts_unnamed() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[names(&[("spare", Some("Spare")), ("osc2", Some("Pad"))])])
        .unwrap();
    running
        .apply_edits(&[remove("spare"), add("spare", "oscillator", json!({}))])
        .unwrap();
    assert_eq!(name_of(&running, "spare"), None);
    running.remove_module("osc2").unwrap();
    assert!(running
        .document()
        .unwrap()
        .modules
        .iter()
        .all(|spec| spec.name.is_none()));
}

#[test]
fn swapping_a_module_keeps_its_name() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[names(&[("osc1", Some("Lead"))])])
        .unwrap();
    let square = json!({ "waveform": "square", "frequency": 440.0 });
    running
        .swap_module("osc1", "oscillator", &square, true)
        .unwrap();
    assert_eq!(name_of(&running, "osc1").as_deref(), Some("Lead"));
}

#[test]
fn a_name_given_while_a_timed_edit_waits_neither_cancels_it_nor_is_lost() {
    let (mut running, pump) = start(BASE);
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let batch = [
        StructuralEdit::AddModule {
            id: "lfo1".into(),
            module_type: "lfo".into(),
            config: json!({ "rate": 2.0 }),
            name: Some("Wobble".into()),
        },
        names(&[("osc2", Some("Pad"))]),
        remove("spare"),
    ];
    let queued = running.apply_edits_at(&batch, &when, None).expect("queued");
    assert_eq!(queued.state, RequestState::Queued, "{queued:?}");
    // While it waits, a module it does not name is named, and one it does.
    running
        .apply_edits(&[names(&[("osc1", Some("Lead")), ("osc2", Some("Strings"))])])
        .unwrap();
    let waiting = running.request_status(&queued.id).expect("remembered");
    assert_eq!(waiting.state, RequestState::Queued, "{waiting:?}");

    pump.render(64);
    running.live.adopt_landed();
    assert_eq!(name_of(&running, "osc1").as_deref(), Some("Lead"));
    // The batch's own names land on top.
    assert_eq!(name_of(&running, "osc2").as_deref(), Some("Pad"));
    assert_eq!(name_of(&running, "lfo1").as_deref(), Some("Wobble"));
}

#[test]
fn a_timed_batch_of_names_alone_is_refused_as_changing_no_structure() {
    let (mut running, _pump) = start(BASE);
    let before = observe(&running);
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let error = running
        .apply_edits_at(&[names(&[("osc1", Some("Lead"))])], &when, None)
        .unwrap_err();
    assert!(error.message.contains("structural"), "{}", error.message);
    assert_eq!(observe(&running), before);
}

#[test]
fn describing_a_module_after_a_timed_edit_lands_reads_its_name_without_waiting() {
    let (mut running, pump) = start(BASE);
    running
        .apply_edits(&[names(&[("osc1", Some("Lead"))])])
        .unwrap();
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    running
        .apply_edits_at(&[remove("spare")], &when, None)
        .expect("queued");
    pump.render(64);
    // Nothing has adopted the landed edit yet: describing catches up under
    // the publisher (which takes the port mirror) before it reads the ports.
    let query = crate::DescribeModuleQuery {
        module_id: Some("osc1".into()),
        ..Default::default()
    };
    let description = running.describe_module(&query).unwrap();
    assert_eq!(description.name.as_deref(), Some("Lead"));
}

fn snapshot_name(running: &RunningInvention, id: &str) -> Option<String> {
    let snapshot = running.full_snapshot();
    let module = snapshot.modules.iter().find(|m| m.info.id == id);
    module.unwrap().info.name.clone()
}

#[test]
fn the_next_snapshot_shows_a_name_edit_and_its_clearing() {
    let (mut running, _pump) = start(BASE);
    assert_eq!(snapshot_name(&running, "osc1"), None);

    running
        .apply_edits(&[names(&[("osc1", Some("Lead")), ("osc2", Some("Pad"))])])
        .unwrap();
    assert_eq!(snapshot_name(&running, "osc1").as_deref(), Some("Lead"));
    assert_eq!(snapshot_name(&running, "osc2").as_deref(), Some("Pad"));

    running.apply_edits(&[names(&[("osc1", None)])]).unwrap();
    assert_eq!(snapshot_name(&running, "osc1"), None);
    assert_eq!(snapshot_name(&running, "osc2").as_deref(), Some("Pad"));
}
