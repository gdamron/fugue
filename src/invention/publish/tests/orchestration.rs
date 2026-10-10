//! The orchestration modules, code and agent, on declared controls: their
//! scalars apply on the audio thread and read back, the agent's edges are
//! counted exactly, its telemetry is read-only, and no audio-thread path
//! (scheduler writes included) takes the lock their control-side strings
//! sit behind.

use super::migrated::{add_locking, block_while_held, read, schedule, write, LockingFactory};
use super::requests::counted_block;
use super::*;
use crate::modules::{AgentControls, CodeControls};
use crate::{ControlSurface, ControlValue};

const ORCHESTRATION: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator" },
        { "id": "agent", "type": "agent", "config": {
            "backend": "test:echo", "prompt": "vary it", "cooldown": 1.0 } },
        { "id": "code", "type": "code", "config": { "tick_rate": 4.0, "script": "1" } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// The concrete controls behind `id`'s surface.
fn controls<T: Clone + 'static>(rig: &Rig, id: &str) -> T {
    let surfaces = rig.live.control_surfaces.lock().unwrap();
    let surface = surfaces[id].as_any().unwrap();
    surface.downcast_ref::<T>().unwrap().clone()
}

fn number(rig: &Rig, id: &str, key: &str) -> f32 {
    match read(rig, id, key) {
        ControlValue::Number(number) => number,
        other => panic!("{id}.{key} is {other:?}"),
    }
}

/// Whether a scheduler writing `id.key` with `value` loads.
fn schedules(rig: &mut Rig, id: &str, key: &str, value: serde_json::Value) -> bool {
    let sched = rig.build(
        "sched",
        "control_scheduler",
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": id, "control": key, "value": value }
        ]}),
    );
    rig.live
        .edit(|change| {
            change.upsert("sched", sched);
            Ok(())
        })
        .is_ok()
}

#[test]
fn code_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(ORCHESTRATION);
    rig.render(1);
    write(&rig, "code", "enabled", false.into());
    write(&rig, "code", "tick_rate", (-2.0).into());
    assert_eq!(read(&rig, "code", "enabled"), true.into(), "pending");
    assert_eq!(read(&rig, "code", "tick_rate"), 4.0.into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "code", "enabled"), false.into());
    assert_eq!(read(&rig, "code", "tick_rate"), 0.0.into(), "clamped");
    write(&rig, "code", "tick_rate", 2000.0.into());
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "code", "tick_rate"), 2000.0.into(), "unbounded");
    let code: CodeControls = controls(&rig, "code");
    assert_eq!((code.enabled(), code.tick_rate()), (false, 2000.0));
    // The strings are control-side: written and read at once.
    write(&rig, "code", "status", "running".into());
    assert_eq!(read(&rig, "code", "status"), "running".into());
    assert!(!schedules(&mut rig, "code", "status", 1.0.into()));
}

#[test]
fn a_scheduler_ramps_code_controls_without_allocating() {
    let mut rig = Rig::new(ORCHESTRATION);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "code", "control": "tick_rate", "value": 8.0, "ramp_steps": 4 },
            { "at_step": 1, "module": "code", "control": "enabled", "value": false }
        ]}),
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let tick_rate = number(&rig, "code", "tick_rate");
    assert!(tick_rate > 4.0 && tick_rate < 8.0, "{tick_rate}");
    assert_eq!(read(&rig, "code", "enabled"), false.into());
}

#[test]
fn a_block_renders_while_the_code_strings_lock_is_held() {
    let mut rig = Rig::new(ORCHESTRATION);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "code", "control": "tick_rate", "value": 8.0, "ramp_steps": 4 }
        ]}),
    );
    let code: CodeControls = controls(&rig, "code");
    write(&rig, "code", "enabled", false.into());
    rig.live.write_input("sched", "clock", 1.0).unwrap();

    let held = block_while_held(&mut rig, || code.hold_state_lock());
    assert_eq!(held, Ok((0, 0)));
    assert_eq!(read(&rig, "code", "enabled"), false.into());
    let tick_rate = number(&rig, "code", "tick_rate");
    assert!(
        tick_rate > 4.0,
        "the ramp wrote under the lock: {tick_rate}"
    );
}

/// The negative control: a module reading the code module's strings each
/// block, as an audio-thread write to them would, is caught waiting on
/// their lock.
#[test]
fn a_block_that_reads_the_code_strings_is_caught() {
    let mut rig = Rig::new(ORCHESTRATION);
    let code: CodeControls = controls(&rig, "code");
    let peeking = code.clone();
    let each_block = move || drop(peeking.get_control("status"));
    add_locking(&mut rig, LockingFactory(Arc::new(each_block)));

    assert!(block_while_held(&mut rig, || ()).is_ok());
    assert!(block_while_held(&mut rig, || code.hold_state_lock()).is_err());
}

#[test]
fn agent_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(ORCHESTRATION);
    rig.render(1);
    write(&rig, "agent", "enabled", false.into());
    write(&rig, "agent", "cooldown", (-2.0).into());
    assert_eq!(read(&rig, "agent", "enabled"), true.into(), "pending");
    assert_eq!(read(&rig, "agent", "cooldown"), 1.0.into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "agent", "enabled"), false.into());
    assert_eq!(read(&rig, "agent", "cooldown"), 0.0.into(), "clamped");
    // The strings are control-side: written and read at once.
    write(&rig, "agent", "prompt", "again".into());
    assert_eq!(read(&rig, "agent", "prompt"), "again".into());
}

#[test]
fn input_edges_are_counted_exactly() {
    let mut rig = Rig::new(ORCHESTRATION);
    rig.render(1);
    let agent: AgentControls = controls(&rig, "agent");
    rig.live.write_input("agent", "trigger", 1.0).unwrap();
    rig.live.write_input("agent", "reset", 1.0).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(counted_block(&mut rig), (0, 0), "held high: one edge");
    rig.live.write_input("agent", "trigger", 0.0).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0));
    rig.live.write_input("agent", "trigger", 1.0).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!((agent.trigger_count(), agent.reset_count()), (2, 1));
}

#[test]
fn agent_telemetry_and_strings_cannot_be_written_or_scheduled() {
    let mut rig = Rig::new(ORCHESTRATION);
    rig.render(1);
    let surface = rig.live.control_surfaces.lock().unwrap()["agent"].clone();
    let refusal = surface.set_control("request_count", 9.0.into());
    assert_eq!(refusal.unwrap_err(), "Control 'request_count' is read-only");
    let refusal = surface.set_control("status", "busy".into());
    assert_eq!(refusal.unwrap_err(), "Control 'status' is read-only");
    // The worker's writer lands it, lock-free.
    let agent: AgentControls = controls(&rig, "agent");
    agent.set_telemetry("request_count", 3.0.into()).unwrap();
    assert_eq!(number(&rig, "agent", "request_count"), 3.0);

    for (key, value) in [
        ("request_count", serde_json::json!(5.0)),
        ("prompt", serde_json::json!(1.0)),
    ] {
        assert!(!schedules(&mut rig, "agent", key, value), "{key}");
    }
    assert!(schedules(&mut rig, "agent", "enabled", true.into()));
}

#[test]
fn a_scheduler_ramps_agent_controls_without_allocating() {
    let mut rig = Rig::new(ORCHESTRATION);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "agent", "control": "cooldown", "value": 5.0, "ramp_steps": 4 },
            { "at_step": 1, "module": "agent", "control": "enabled", "value": false }
        ]}),
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let cooldown = number(&rig, "agent", "cooldown");
    assert!(cooldown > 1.0 && cooldown < 5.0, "{cooldown}");
    assert_eq!(read(&rig, "agent", "enabled"), false.into());
}

#[test]
fn a_block_renders_while_the_agent_strings_lock_is_held() {
    let mut rig = Rig::new(ORCHESTRATION);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "agent", "control": "cooldown", "value": 5.0, "ramp_steps": 4 }
        ]}),
    );
    let agent: AgentControls = controls(&rig, "agent");
    write(&rig, "agent", "enabled", false.into());
    rig.live.write_input("agent", "trigger", 1.0).unwrap();
    rig.live.write_input("sched", "clock", 1.0).unwrap();
    rig.live.write_input("agent", "reset", 1.0).unwrap();

    let held = block_while_held(&mut rig, || agent.hold_state_lock());
    assert_eq!(held, Ok((0, 0)));
    assert_eq!(read(&rig, "agent", "enabled"), false.into());
    assert_eq!((agent.trigger_count(), agent.reset_count()), (1, 1));
    let cooldown = number(&rig, "agent", "cooldown");
    assert!(cooldown > 1.0, "the ramp wrote under the lock: {cooldown}");
}

/// The negative control, for the agent's strings.
#[test]
fn a_block_that_reads_the_agent_strings_is_caught() {
    let mut rig = Rig::new(ORCHESTRATION);
    let agent: AgentControls = controls(&rig, "agent");
    let peeking = agent.clone();
    let each_block = move || drop(peeking.get_control("prompt"));
    add_locking(&mut rig, LockingFactory(Arc::new(each_block)));

    assert!(block_while_held(&mut rig, || ()).is_ok());
    assert!(block_while_held(&mut rig, || agent.hold_state_lock()).is_err());
}
