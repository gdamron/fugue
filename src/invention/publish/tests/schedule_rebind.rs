//! A change that rebuilds a module a surviving scheduler targets hands the
//! scheduler its schedule re-resolved against the change, adopted in the
//! block that installs it, so scheduled values reach the new instance. One
//! that removes such a module is refused, with nothing changed.

use super::migrated::{block_while_held, read, write};
use super::requests::counted_block;
use super::schedule_payload::{clocked, scheduler};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::ControlValue;

const STEPS: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 0, "module": "o", "control": "frequency", "value": 220.0 },
            { "at_step": 1, "module": "o", "control": "frequency", "value": 330.0 }
        ] } },
        { "id": "o", "type": "oscillator", "config": { "frequency": 440.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "o", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn frequency(rig: &Rig) -> ControlValue {
    read(rig, "o", "frequency")
}

/// Advances `sched` one step: a block with its clock high, then one low,
/// neither allocating nor freeing.
fn step(rig: &mut Rig) {
    assert_eq!(clocked(rig, "sched", 1.0), (0, 0));
    assert_eq!(clocked(rig, "sched", 0.0), (0, 0));
}

/// `STEPS` with step 0 played.
fn stepped() -> Rig {
    let mut rig = Rig::new(STEPS);
    rig.render(1);
    step(&mut rig);
    assert_eq!(frequency(&rig), ControlValue::Number(220.0));
    rig
}

#[test]
fn a_scheduled_value_reaches_the_module_that_replaces_its_target() {
    let mut rig = stepped();
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    assert_eq!(
        frequency(&rig),
        ControlValue::Number(440.0),
        "a new instance"
    );
    // The installing block adopts the schedule waiting on no lock.
    let controls = scheduler(&rig);
    assert_eq!(
        block_while_held(&mut rig, || controls.held()),
        Ok((0, 0)),
        "the installing block"
    );
    step(&mut rig);
    assert_eq!(frequency(&rig), ControlValue::Number(330.0));
    let (_, _, frees) = allocator_events(|| rig.live.reclaim());
    assert!(frees >= 1, "the reclaimer frees the schedule replaced");
}

#[test]
fn removing_a_scheduled_target_is_refused_with_nothing_changed() {
    let mut rig = stepped();
    let before = rig.generation_and_applied();
    let error = match rig.live.remove_module("o") {
        Ok(_) => panic!("removing a scheduled target is refused"),
        Err(error) => error,
    };
    let GraphCommandError::ModuleBuildFailed(message) = error else {
        panic!("{error:?}");
    };
    assert!(
        message.contains("'sched'") && message.contains("'o'"),
        "{message}"
    );
    assert_eq!(rig.generation_and_applied(), before);
    assert_eq!(rig.module_ids(), ["sched", "o", "dac"]);
    step(&mut rig);
    assert_eq!(frequency(&rig), ControlValue::Number(330.0));
}

#[test]
fn a_schedule_written_while_a_change_is_prepared_resolves_against_the_change() {
    let mut rig = stepped();
    let mut change = rig.live.begin();
    let config = serde_json::json!({ "frequency": 440.0 });
    change.upsert("o", rig.build("o", "oscillator", config));
    let prepared = change.prepare().unwrap();
    // Resolved against the directory as it is: the instance the change
    // replaces. The change's own rebind is stale now.
    let next = r#"[{ "at_step": 1, "module": "o", "control": "frequency", "value": 550.0 }]"#;
    write(&rig, "sched", "schedule", next.into());
    rig.live.commit_with(prepared, |_| {}).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    step(&mut rig);
    assert_eq!(frequency(&rig), ControlValue::Number(550.0));
}

/// The rebound schedule travels in the publication, not as a request
/// behind it: a control request queue full to its edit reserve takes the
/// edit, and the block that installs it adopts the schedule.
#[test]
fn a_full_request_queue_still_hands_the_scheduler_its_rebound_schedule() {
    let mut rig = stepped();
    let surface = rig.live.control_surfaces.lock().unwrap()["o"].clone();
    let mut queued = 0;
    while surface.set_control("frequency", 220.0.into()).is_ok() {
        queued += 1;
    }
    assert!(queued > 0, "the queue fills");
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    step(&mut rig);
    assert_eq!(frequency(&rig), ControlValue::Number(330.0));
}

/// A schedule written between a change's preparation and its commit that
/// targets a module the change removes is caught at the commit, under the
/// publisher, and the change refused with nothing changed.
#[test]
fn a_schedule_retargeted_onto_a_module_a_prepared_change_removes_refuses_it() {
    let doc = STEPS.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "p", "type": "oscillator", "config": { "frequency": 110.0 } },
        { "id": "dac""#,
    );
    let mut rig = Rig::new(&doc);
    rig.render(1);
    let mut change = rig.live.begin();
    change.remove("p");
    let prepared = change.prepare().unwrap();
    let onto_p = r#"[{ "at_step": 1, "module": "p", "control": "frequency", "value": 55.0 }]"#;
    write(&rig, "sched", "schedule", onto_p.into());
    let before = rig.generation_and_applied();
    let Err(GraphCommandError::ModuleBuildFailed(message)) = rig.live.commit_with(prepared, |_| {})
    else {
        panic!("refused");
    };
    assert!(message.contains("'p'"), "{message}");
    assert_eq!(rig.generation_and_applied(), before);
    assert_eq!(rig.module_ids(), ["sched", "o", "p", "dac"]);
}
