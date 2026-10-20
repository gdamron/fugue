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

/// `STEPS` with a second oscillator, `p`, its schedule left alone.
fn with_p() -> String {
    STEPS.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "p", "type": "oscillator", "config": { "frequency": 110.0 } },
        { "id": "dac""#,
    )
}

/// A schedule write refused for want of room, as requests ahead of an edit
/// behind a full store are, never leaves the scheduler playing a schedule
/// bound to an instance the edit replaced: until the scheduler reports
/// adopting the schedule written last, a change rebinds it whatever it
/// touches, and the installing block adopts the schedule written last.
#[test]
fn a_schedule_write_refused_ahead_of_a_change_leaves_no_target_replaced() {
    use super::requests::submit;
    use crate::control_request::When;
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let mut rig = Rig::new(&with_p());
    rig.render(1);
    step(&mut rig);
    // Fill the pending store with far-future writes to `p`.
    let far = rig.graph.current_sample + 1_000_000;
    let mut next = 0;
    for _ in 0..PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
        for _ in 0..REQUEST_QUEUE_CAPACITY {
            submit(&rig, "p", 0, 110.0, When::AtSample(far + next));
            next += 1;
        }
        assert_eq!(counted_block(&mut rig), (0, 0), "filling");
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
    // A schedule that no longer targets `o`, then a change replacing `o`:
    // the write is refused so the change installs.
    let onto_p = r#"[{ "at_step": 1, "module": "p", "control": "frequency", "value": 55.0 }]"#;
    write(&rig, "sched", "schedule", onto_p.into());
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    step(&mut rig);
    assert_eq!(read(&rig, "p", "frequency"), ControlValue::Number(55.0));
    assert_eq!(frequency(&rig), ControlValue::Number(440.0));
}

/// A schedule a change writes orders the processing it installs: retargeted
/// onto the clock driving it, the scheduler joins that clock's feedback
/// group in the same block, so a tempo it schedules is not a block late.
#[test]
fn a_change_compiles_against_the_schedule_it_writes() {
    let doc = STEPS.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "c", "type": "clock", "config": { "bpm": 120.0 } },
        { "id": "dac""#,
    );
    let doc = doc.replace(
        r#""connections": ["#,
        r#""connections": [
        { "from": "c", "from_port": "beat", "to": "sched", "to_port": "clock" },"#,
    );
    let mut rig = Rig::new(&doc);
    rig.render(1);
    let group = |rig: &Rig, id: &str| {
        let idx = rig.graph.modules.get_index_of(id).unwrap();
        let groups = &rig.graph.process_groups;
        groups
            .iter()
            .position(|g| g.members.contains(&idx))
            .unwrap()
    };
    assert_ne!(group(&rig, "sched"), group(&rig, "c"));
    let onto_c = r#"[{ "at_step": 1, "module": "c", "control": "bpm", "value": 90.0 }]"#;
    let mut change = rig.live.begin();
    let config = serde_json::json!({ "frequency": 440.0 });
    change.upsert("o", rig.build("o", "oscillator", config));
    change.reschedule("sched", onto_c);
    let prepared = change.prepare().unwrap();
    rig.live.commit_with(prepared, |_| {}).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    assert_eq!(group(&rig, "sched"), group(&rig, "c"));
    let feedback = rig.graph.process_groups[group(&rig, "c")].feedback;
    assert!(feedback, "a feedback group");
}

/// A ramp in flight when its target is rebuilt carries on, onto the new
/// instance, from the step it had reached.
#[test]
fn a_ramp_in_flight_carries_on_onto_the_module_that_replaces_its_target() {
    let ramp = r#"[{ "at_step": 0, "module": "o", "control": "frequency", "value": 200.0, "ramp_steps": 4 }]"#;
    let mut rig = Rig::new(STEPS);
    rig.render(1);
    write(&rig, "sched", "schedule", ramp.into());
    step(&mut rig); // step 0: the ramp starts from 440
    step(&mut rig); // step 1
    let ControlValue::Number(reached) = frequency(&rig) else {
        panic!("a number");
    };
    assert!(reached < 440.0 && reached > 200.0, "{reached}");
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    step(&mut rig); // step 2
    let ControlValue::Number(carried) = frequency(&rig) else {
        panic!("a number");
    };
    assert!(carried < reached && carried > 200.0, "{carried}");
    step(&mut rig); // step 3
    step(&mut rig); // step 4: the ramp arrives
    assert_eq!(frequency(&rig), ControlValue::Number(200.0));
}

/// A ramp on a target the change leaves alone carries on when the change
/// rebuilds another target of the same schedule, and reaches its endpoint.
#[test]
fn a_ramp_on_a_surviving_target_carries_on_when_another_target_is_rebuilt() {
    let both = r#"[
        { "at_step": 0, "module": "p", "control": "frequency", "value": 55.0, "ramp_steps": 4 },
        { "at_step": 1, "module": "o", "control": "frequency", "value": 330.0 }
    ]"#;
    let mut rig = Rig::new(&with_p());
    rig.render(1);
    write(&rig, "sched", "schedule", both.into());
    step(&mut rig); // step 0: p's ramp starts from 110
    step(&mut rig); // step 1
    let ControlValue::Number(reached) = read(&rig, "p", "frequency") else {
        panic!("a number");
    };
    assert!(reached < 110.0 && reached > 55.0, "{reached}");
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0), "the installing block");
    step(&mut rig); // step 2
    let ControlValue::Number(carried) = read(&rig, "p", "frequency") else {
        panic!("a number");
    };
    assert!(carried < reached && carried > 55.0, "{carried}");
    step(&mut rig); // step 3
    step(&mut rig); // step 4: the ramp arrives
    assert_eq!(read(&rig, "p", "frequency"), ControlValue::Number(55.0));
}
