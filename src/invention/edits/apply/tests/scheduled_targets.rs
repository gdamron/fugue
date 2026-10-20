//! Batches and reloads that rebuild or remove a module a running scheduler
//! targets: the scheduler's values reach the module that replaces it, and
//! removing one is refused unless the change writes the scheduler a
//! schedule that no longer targets it.

use super::{add, base_with, doc, remove, set, start};
use crate::invention::manual_backend::Pump;
use crate::invention::runtime::RunningInvention;
use crate::rpc::RpcErrorCode;
use crate::ControlValue;

/// `spare` to 220 at step 0, then 330 at step 1.
const STEPS: &str = r#"[
    { "at_step": 0, "module": "spare", "control": "frequency", "value": 220.0 },
    { "at_step": 1, "module": "spare", "control": "frequency", "value": 330.0 }
]"#;

const TARGETS_OSC1: &str =
    r#"[{ "at_step": 1, "module": "osc1", "control": "frequency", "value": 220.0 }]"#;

/// BASE with a scheduler `sched` whose config holds `schedule` as text.
fn scheduled(schedule: &str) -> String {
    let config = serde_json::json!({ "schedule": schedule });
    base_with(&format!(
        r#"{{ "id": "sched", "type": "control_scheduler", "config": {config} }}"#
    ))
}

/// Advances `sched` one step.
fn step(running: &RunningInvention, pump: &Pump) {
    for level in [1.0, 0.0] {
        running.live.write_input("sched", "clock", level).unwrap();
        pump.render(1);
    }
}

fn frequency(running: &RunningInvention, id: &str) -> ControlValue {
    running.get_control(id, "frequency").unwrap()
}

#[test]
fn a_batch_replacing_a_scheduled_target_hands_its_values_to_the_new_instance() {
    let (mut running, pump) = start(&scheduled(STEPS));
    step(&running, &pump);
    assert_eq!(frequency(&running, "spare"), ControlValue::Number(220.0));
    running
        .apply_edits(&[
            remove("spare"),
            add(
                "spare",
                "oscillator",
                serde_json::json!({ "frequency": 3.0 }),
            ),
        ])
        .expect("the batch commits");
    step(&running, &pump);
    assert_eq!(frequency(&running, "spare"), ControlValue::Number(330.0));
}

#[test]
fn a_batch_removing_a_scheduled_target_commits_only_with_a_schedule_that_drops_it() {
    let (mut running, pump) = start(&scheduled(STEPS));
    step(&running, &pump);
    let error = running
        .apply_edits(&[remove("spare")])
        .expect_err("the scheduler still targets it");
    assert_eq!(error.code, RpcErrorCode::ModuleBuildFailed);

    running
        .apply_edits(&[
            remove("spare"),
            set(
                "sched",
                "schedule",
                ControlValue::String(TARGETS_OSC1.into()),
            ),
        ])
        .expect("the batch commits");
    step(&running, &pump);
    assert_eq!(frequency(&running, "osc1"), ControlValue::Number(220.0));
}

#[test]
fn a_reload_removing_a_scheduled_target_commits_only_with_a_schedule_that_drops_it() {
    let (mut running, pump) = start(&scheduled(STEPS));
    step(&running, &pump);
    let without = |schedule: &str| {
        let mut document = doc(&scheduled(schedule));
        document.modules.retain(|spec| spec.id != "spare");
        document
    };
    let refused = running.reload(without(STEPS));
    assert!(refused.is_err(), "the scheduler still targets it");
    assert!(running.get_control("spare", "frequency").is_ok());

    running
        .reload(without(TARGETS_OSC1))
        .expect("the reload commits");
    step(&running, &pump);
    assert_eq!(frequency(&running, "osc1"), ControlValue::Number(220.0));
}
