//! Committing a batch: another edit landing mid-batch, a write that fails
//! only when made, and a publication that installs without allocating.

use super::scripted::Step;
use super::*;
use crate::alloc_counter::allocator_events;
use crate::rpc::{RpcErrorCode, MODULE_ERROR_BYTES};

/// A hook replacing `spare` in place through the live graph, as a script's
/// edit would, so the graph moves under the batch.
fn replace_spare(running: &RunningInvention, registry: &ModuleRegistry) -> Step {
    let live = running.live.clone();
    let registry = registry.clone();
    Step::Run(Box::new(move || {
        live.add_module(
            &registry,
            SAMPLE_RATE,
            "spare",
            "oscillator",
            &json!({ "frequency": 6.0 }),
        )
        .unwrap();
    }))
}

/// Adds `t` with a changed config, so the commit builds it (and runs its
/// script) on every attempt, and removes the spare the hook replaces.
fn moving_batch() -> Vec<StructuralEdit> {
    vec![
        add("t", SCRIPTED, json!({})),
        set("t", "level", number(0.5)),
        remove("spare"),
    ]
}

#[test]
fn a_batch_the_graph_moves_under_is_planned_again_and_commits() {
    let scripted = Scripted::default();
    let registry = scripted.registry();
    let (mut running, pump) = start_with(registry.clone(), BASE);
    // Checked, validated, then the first attempt's build lets another edit
    // publish; the second attempt's build passes.
    let hook = replace_spare(&running, &registry);
    scripted.then(None).then(None).then(Some(hook));
    let events = Events::listen(&running);
    let generation = running.live.generation();

    let report = running
        .apply_edits(&moving_batch())
        .expect("the batch commits");
    assert_eq!(scripted.builds(), 4);
    assert_eq!(report.added, ["t"]);
    assert_eq!(report.removed, ["spare"]);
    // The other edit's publication, then the batch's.
    assert_eq!(running.live.generation(), generation + 2);
    assert_eq!(running.get_control("t", "level").unwrap(), number(0.5));
    assert_eq!(events.control_changes(), [change("t", "level", 0.5)]);
    assert!(!running.state.lock().unwrap().modules.contains_key("spare"));
    pump.render(1);
}

#[test]
fn a_batch_the_graph_keeps_moving_under_is_refused_after_three_attempts() {
    let scripted = Scripted::default();
    let registry = scripted.registry();
    let (mut running, pump) = start_with(registry.clone(), BASE);
    scripted.then(None).then(None);
    for _ in 0..3 {
        scripted.then(Some(replace_spare(&running, &registry)));
    }
    let events = Events::listen(&running);

    let error = running
        .apply_edits(&moving_batch())
        .expect_err("the batch is refused");
    assert_eq!(error.code, RpcErrorCode::Internal);
    assert!(error.edit.is_none());
    assert_eq!(scripted.builds(), 5);

    // Only the other edits landed.
    let state = running.state.lock().unwrap();
    assert!(state.modules.contains_key("spare"));
    assert!(!state.modules.contains_key("t"));
    drop(state);
    assert!(running
        .document()
        .unwrap()
        .modules
        .iter()
        .all(|spec| spec.id != "t"));
    assert_eq!(events.count(), 0);
    pump.render(1);
}

#[test]
fn a_write_that_fails_when_made_is_reported_and_the_batch_still_commits() {
    // One ASCII byte shifts every two-byte "é", so the cut lands
    // mid-character and has to back off by one.
    let long = format!("a{}", "é".repeat(MODULE_ERROR_BYTES));
    let flaky = r#"{ "id": "flaky", "type": "scripted", "config": { "level": 0.25 } }"#;
    // The running module refuses writes its authored config would take, so
    // the batch passes its checks and the write fails only when made.
    let scripted = Scripted::default();
    scripted.then(Some(Step::Refuse(long.clone())));
    let (mut running, pump) = start_with(scripted.registry(), &base_with(flaky));
    let events = Events::listen(&running);

    let report = running
        .apply_edits(&[
            set("flaky", "level", number(0.75)),
            set("osc1", "frequency", number(220.0)),
            add("v", "oscillator", json!({})),
        ])
        .expect("the batch commits");
    assert_eq!(report.added, ["v"]);
    assert_eq!(report.controls_written, written(&[("osc1", "frequency")]));
    assert_eq!(report.controls_failed.len(), 1);
    let failure = &report.controls_failed[0];
    assert_eq!(
        (
            failure.edit_index,
            failure.module_id.as_str(),
            failure.key.as_str()
        ),
        (0, "flaky", "level")
    );
    assert_eq!(failure.error.len(), MODULE_ERROR_BYTES - 1);
    assert!(long.starts_with(failure.error.as_str()));

    // The module kept its value, the document records that value, and the
    // failed write was not announced.
    assert_eq!(running.get_control("flaky", "level").unwrap(), number(0.25));
    assert_eq!(config_of(&running, "flaky")["level"], json!(0.25));
    assert_eq!(config_of(&running, "osc1")["frequency"], json!(220));
    // The stored configs agree with the document, so a later reload diffs
    // against what each module plays.
    {
        let state = running.state.lock().unwrap();
        assert_eq!(state.modules["flaky"].config["level"], json!(0.25));
        assert_eq!(state.modules["osc1"].config["frequency"], json!(220));
    }
    assert_eq!(
        events.control_changes(),
        [change("osc1", "frequency", 220.0)]
    );
    pump.render(1);
}

#[test]
fn a_batch_publication_installs_without_allocating() {
    let (mut running, pump) = start(BASE);
    pump.render(2);
    running
        .apply_edits(&[
            add("v", "oscillator", json!({ "frequency": 330.0 })),
            connect("v", "audio", "dac", "audio"),
            connect("v", "audio", "osc1", "fm"),
            remove("spare"),
            remove("osc2"),
            add("osc2", "oscillator", json!({ "waveform": "square" })),
            connect("osc2", "audio", "dac", "audio"),
            set("osc1", "frequency", number(220.0)),
        ])
        .expect("the batch commits");

    // The install block, then an ordinary one.
    let (generation, applied) = publications(&running);
    for block in 0..2 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "block {block}");
    }
    assert_eq!(publications(&running), (generation, applied + 1));
}

#[test]
fn a_write_the_module_always_refuses_is_refused_before_anything_applies() {
    let refusing = r#"{ "id": "refusing", "type": "scripted", "config": { "level": 0.25, "error": "never" } }"#;
    let scripted = Scripted::default();
    let (mut running, _pump) = start_with(scripted.registry(), &base_with(refusing));
    let before = running.document().unwrap();
    let error = running
        .apply_edits(&[
            set("osc1", "frequency", number(220.0)),
            set("refusing", "level", number(0.75)),
        ])
        .expect_err("the module refuses every write");
    assert_eq!(error.edit.as_ref().unwrap().index, 1);
    assert_eq!(running.document().unwrap(), before);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(440.0)
    );
}
