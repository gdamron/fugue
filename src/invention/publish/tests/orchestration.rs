//! The orchestration modules, code and agent, on declared controls: their
//! scalars apply on the audio thread and read back, and no audio-thread
//! path (scheduler writes included) takes the lock their control-side
//! strings sit behind.

use super::migrated::{add_locking, block_while_held, read, schedule, write, LockingFactory};
use super::requests::counted_block;
use super::*;
use crate::modules::CodeControls;
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
