//! Every refusal leaves the running invention as it was: no module,
//! connection, control, document, publication or event changes, and the
//! audio thread plays on as if nothing was sent.

use super::scripted::Step;
use super::*;
use crate::invention::edits::apply::plan::guard;
use crate::invention::edits::EditedCandidate;
use crate::invention::reload::ReloadPlan;
use crate::rpc::{EditFailureReason, RpcError, RpcErrorCode};

/// Applies `edits`, expecting a refusal, and checks that it changed
/// nothing; returns the refusal.
fn refused(running: &mut RunningInvention, edits: &[StructuralEdit]) -> RpcError {
    let events = Events::listen(running);
    let before = observe(running);
    let error = running
        .apply_edits(edits)
        .expect_err("the batch is refused");
    assert_eq!(observe(running), before, "{error:?}");
    assert_eq!(events.count(), 0, "{error:?}");
    error
}

fn at_edit(error: &RpcError) -> (usize, EditFailureReason) {
    assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{error:?}");
    let edit = error.edit.as_ref().expect("an edit detail");
    (edit.index, edit.reason)
}

#[test]
fn per_edit_and_control_value_refusals_change_nothing() {
    let (mut running, pump) = start(BASE);
    let (_twin, twin_pump) = start(BASE);
    assert_eq!(pump.render(3), twin_pump.render(3));

    let error = refused(&mut running, &[]);
    assert_eq!(error.code, RpcErrorCode::InvalidRequest);
    assert!(error.edit.is_none());

    // An edit that cannot apply to the candidate.
    let error = refused(
        &mut running,
        &[
            add("v", "oscillator", json!({})),
            connect("v", "audio", "dac", "nope"),
        ],
    );
    assert_eq!(at_edit(&error), (1, EditFailureReason::UnknownPort));

    // A value the survivor's setter refuses, even one a later edit
    // overwrites.
    let error = refused(
        &mut running,
        &[
            set("osc1", "type", ControlValue::String("square".into())),
            set("osc1", "type", ControlValue::String("bogus".into())),
            set("osc1", "type", ControlValue::String("sine".into())),
        ],
    );
    assert_eq!(at_edit(&error), (1, EditFailureReason::InvalidControlValue));

    // A value an added module refuses.
    let error = refused(
        &mut running,
        &[
            add("v", "oscillator", json!({})),
            set("v", "type", ControlValue::String("bogus".into())),
        ],
    );
    assert_eq!(at_edit(&error), (1, EditFailureReason::InvalidControlValue));

    // A value written into a config the module type parses: refused at its
    // edit before the validation build could fail on it with no index.
    let error = refused(
        &mut running,
        &[
            add("l", "lfo", json!({})),
            set("l", "waveform", ControlValue::String("bogus".into())),
        ],
    );
    assert_eq!(at_edit(&error), (1, EditFailureReason::InvalidControlValue));

    assert_eq!(pump.render(10), twin_pump.render(10));
}

#[test]
fn an_edited_invention_that_does_not_build_is_refused_with_no_edit_to_blame() {
    // The scheduler targets the spare: removing it is a valid edit, but
    // the edited invention no longer builds.
    let base = base_with(
        r#"{ "id": "sched", "type": "control_scheduler", "config": {
            "schedule": [{ "at": 0, "module": "spare", "control": "frequency", "value": 1.0 }]
        } }"#,
    );
    let (mut running, pump) = start(&base);
    let error = refused(&mut running, &[remove("spare")]);
    assert_eq!(error.code, RpcErrorCode::ModuleBuildFailed);
    assert!(error.edit.is_none());
    assert!(error.message.contains("spare"), "{}", error.message);
    pump.render(1);
}

#[test]
fn a_module_that_fails_to_build_while_the_change_is_prepared_changes_nothing() {
    let scripted = Scripted::default();
    // Checked, validated, then failing when the commit builds it from its
    // final config.
    scripted.then(None).then(None).then(Some(Step::Fail));
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let error = refused(
        &mut running,
        &[
            add("t", SCRIPTED, json!({})),
            set("t", "level", number(0.5)),
        ],
    );
    assert_eq!(error.code, RpcErrorCode::ModuleBuildFailed);
    assert!(error.edit.is_none());
    assert_eq!(scripted.builds(), 3);
    pump.render(1);
}

#[test]
fn a_schedule_that_fails_to_attach_while_the_change_is_prepared_changes_nothing() {
    let scripted = Scripted::default();
    // The target resolves when checked and validated, then not when the
    // commit builds it, so the new scheduler cannot attach.
    scripted.then(None).then(None).then(Some(Step::Hide));
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let schedule = json!({
        "schedule": [{ "at": 0, "module": "t", "control": "level", "value": 1.0 }]
    });
    let error = refused(
        &mut running,
        &[
            add("t", SCRIPTED, json!({})),
            set("t", "level", number(0.5)),
            add("sched", "control_scheduler", schedule),
        ],
    );
    assert_eq!(error.code, RpcErrorCode::ModuleBuildFailed);
    assert!(error.edit.is_none());
    assert!(error.message.contains("schedule"), "{}", error.message);
    pump.render(1);
}

#[test]
fn a_plan_that_reaches_past_the_batch_is_refused() {
    // Another edit adds `late` while the batch is validated. The candidate
    // was made without it, so its plan would remove a module no edit
    // names.
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let live = running.live.clone();
    scripted.then(None).then(Some(Step::Run(Box::new(move || {
        live.add_module(SAMPLE_RATE, "late", "oscillator", &json!({}))
            .unwrap();
    }))));
    let events = Events::listen(&running);

    let error = running
        .apply_edits(&[add("t", SCRIPTED, json!({}))])
        .expect_err("the batch is refused");
    assert_eq!(error.code, RpcErrorCode::Internal);
    assert!(error.message.contains("'late'"), "{}", error.message);

    // Only the other edit landed.
    let state = running.state.lock().unwrap();
    assert!(state.modules.contains_key("late"));
    assert!(!state.modules.contains_key("t"));
    drop(state);
    let document = running.document().unwrap();
    assert!(document.modules.iter().any(|spec| spec.id == "late"));
    assert!(document.modules.iter().all(|spec| spec.id != "t"));
    assert_eq!(events.count(), 0);
    pump.render(1);
}

fn candidate(named: &[&str], replaced: &[&str]) -> EditedCandidate {
    EditedCandidate {
        document: doc(BASE),
        control_writes: Vec::new(),
        named_modules: named.iter().map(|id| id.to_string()).collect(),
        replaced: replaced.iter().map(|id| id.to_string()).collect(),
    }
}

fn spec(id: &str) -> crate::invention::format::ModuleSpec {
    doc(BASE)
        .modules
        .into_iter()
        .find(|spec| spec.id == id)
        .unwrap()
}

#[test]
fn the_guard_names_the_first_module_the_batch_does_not_name() {
    let wired = |from: &str, to: &str| RuntimeConnectionInfo {
        from: from.into(),
        from_port: "audio".into(),
        to: to.into(),
        to_port: "fm".into(),
    };
    let plans: Vec<(ReloadPlan, &str)> = vec![
        (
            ReloadPlan {
                removed: vec!["spare".into()],
                ..ReloadPlan::default()
            },
            "spare",
        ),
        (
            ReloadPlan {
                removed_connections: vec![wired("osc1", "osc2")],
                ..ReloadPlan::default()
            },
            "osc2",
        ),
        (
            ReloadPlan {
                control_updates: vec![("osc2".into(), "frequency".into(), number(1.0))],
                ..ReloadPlan::default()
            },
            "osc2",
        ),
    ];
    for (plan, module) in plans {
        let error = guard(&plan, &candidate(&["osc1"], &[])).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::Internal);
        assert!(
            error.message.contains(&format!("'{module}'")),
            "{}",
            error.message
        );
    }

    // A rebuild needs a replacing edit, not only a name.
    let rebuilt = ReloadPlan {
        swapped: vec![spec("osc1")],
        ..ReloadPlan::default()
    };
    let error = guard(&rebuilt, &candidate(&["osc1"], &[])).unwrap_err();
    assert!(
        error.message.contains("rebuild module 'osc1'"),
        "{}",
        error.message
    );
    assert!(guard(&rebuilt, &candidate(&["osc1"], &["osc1"])).is_ok());
}
