//! A write that passes every check but fails when made: the retained
//! document keeps what it authored for the control, never a value a
//! performed write left playing.

use super::*;
use crate::ControlWriteIntent;

const FLAKY: &str = r#"{ "id": "flaky", "type": "scripted", "config": { "level": 0.25 } }"#;

/// Starts `module` beside BASE's, then performs `level` 0.5 on `flaky`, as
/// a live gesture does, and makes every write to it fail from then on.
fn performed(module: &str) -> (Scripted, RunningInvention, Pump) {
    let scripted = Scripted::default();
    let (running, pump) = start_with(scripted.registry(), &base_with(module));
    let snapshot = running.snapshot();
    let perform = ControlWriteIntent::Perform;
    (snapshot.set_control_with_intent("flaky", "level", number(0.5), perform)).unwrap();
    pump.block();
    assert_eq!(running.get_control("flaky", "level").unwrap(), number(0.5));
    scripted.refuse_writes("drifted");
    (scripted, running, pump)
}

fn stored_level(running: &RunningInvention) -> Option<serde_json::Value> {
    let state = running.state.lock().unwrap();
    state.modules["flaky"].config.get("level").cloned()
}

#[test]
fn a_batch_write_failing_when_made_leaves_the_authored_value_not_the_performed_one() {
    let (_scripted, mut running, pump) = performed(FLAKY);

    let report = running
        .apply_edits(&[set("flaky", "level", number(0.75))])
        .expect("the batch commits");

    assert_eq!(report.controls_failed.len(), 1);
    // The module plays what was performed; the document keeps what was
    // authored, and so does the stored config, so reloading the original
    // leaves the performance alone.
    assert_eq!(running.get_control("flaky", "level").unwrap(), number(0.5));
    assert_eq!(config_of(&running, "flaky")["level"], json!(0.25));
    assert_eq!(stored_level(&running), Some(json!(0.25)));
    pump.render(1);
}

#[test]
fn a_batch_write_failing_to_a_control_the_document_never_authored_leaves_it_unauthored() {
    let unset = r#"{ "id": "flaky", "type": "scripted", "config": {} }"#;
    let (_scripted, mut running, pump) = performed(unset);

    let report = running
        .apply_edits(&[set("flaky", "level", number(0.75))])
        .expect("the batch commits");

    assert_eq!(report.controls_failed.len(), 1);
    assert_eq!(running.get_control("flaky", "level").unwrap(), number(0.5));
    assert_eq!(config_of(&running, "flaky").get("level"), None);
    pump.render(1);
}

#[test]
fn a_reload_write_failing_when_made_leaves_the_authored_value_not_the_performed_one() {
    let (_scripted, mut running, pump) = performed(FLAKY);
    let louder = base_with(&FLAKY.replace("0.25", "0.75"));

    let report = running.reload(doc(&louder)).expect("the reload commits");

    let failed: Vec<_> = (report.controls_failed.iter())
        .map(|failure| format!("{}.{}", failure.module_id, failure.key))
        .collect();
    assert_eq!(failed, ["flaky.level"]);
    assert_eq!(running.get_control("flaky", "level").unwrap(), number(0.5));
    assert_eq!(config_of(&running, "flaky")["level"], json!(0.25));
    // Reloading the louder document again tries the write again.
    assert_eq!(stored_level(&running), Some(json!(0.25)));
    pump.render(1);
}
