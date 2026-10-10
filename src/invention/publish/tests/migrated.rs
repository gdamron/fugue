//! Module types moved onto declared controls after the pilots: their writes
//! and ramps apply on the audio thread without allocating, and a block
//! renders while another thread holds every control-side lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use super::requests::counted_block;
use super::*;
use crate::ControlValue;

const REVERB: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } },
        { "id": "verb", "type": "reverb", "config": { "wet": 0.5 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "verb", "to_port": "audio_left" },
        { "from": "verb", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
        { "from": "verb", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

pub(super) fn read(rig: &Rig, id: &str, key: &str) -> ControlValue {
    let surfaces = rig.live.control_surfaces.lock().unwrap();
    surfaces[id].get_control(key).unwrap()
}

pub(super) fn write(rig: &Rig, id: &str, key: &str, value: ControlValue) {
    let surface = rig.live.control_surfaces.lock().unwrap()[id].clone();
    surface.set_control(key, value).unwrap();
}

/// Adds a control scheduler `sched` running `schedule` to the rig.
pub(super) fn schedule(rig: &mut Rig, schedule: serde_json::Value) {
    let scheduler = rig.build("sched", "control_scheduler", schedule);
    rig.live
        .edit(|change| {
            change.upsert("sched", scheduler);
            Ok(())
        })
        .unwrap();
    rig.render(1);
}

/// One block, counted as [`counted_block`] does, rendered while another
/// thread holds every lock a control thread takes on the live graph. A
/// block that waited for one would only finish once the holder gives up,
/// seconds later, and fail.
pub(super) fn counted_block_with_control_locks_held(rig: &mut Rig) -> (usize, usize) {
    let live = &rig.live;
    let (publisher, pending, state) = (&live.publisher, &live.pending, &live.state);
    let (surfaces, ports) = (&live.control_surfaces, &live.module_ports);
    let released = AtomicBool::new(false);
    let (held, holding) = mpsc::channel();
    let (done, finished) = mpsc::channel::<()>();
    thread::scope(|scope| {
        let released = &released;
        scope.spawn(move || {
            let locks = (
                publisher.lock().unwrap(),
                pending.lock().unwrap(),
                state.lock().unwrap(),
                surfaces.lock().unwrap(),
                ports.lock().unwrap(),
            );
            held.send(()).unwrap();
            let _ = finished.recv_timeout(Duration::from_secs(10));
            released.store(true, Ordering::SeqCst);
            drop(locks);
        });
        holding.recv().unwrap();
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        let graph = &mut rig.graph;
        let ((), allocs, frees) = allocator_events(|| graph.process_block(&mut left, &mut right));
        let waited = released.load(Ordering::SeqCst);
        done.send(()).unwrap();
        assert!(!waited, "the block waited for a control-side lock");
        (allocs, frees)
    })
}

#[test]
fn reverb_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(REVERB);
    rig.render(1);
    write(&rig, "verb", "wet", 2.0.into());
    write(&rig, "verb", "room_size", 0.25.into());
    write(&rig, "verb", "freeze", true.into());
    assert_eq!(read(&rig, "verb", "wet"), 0.5.into(), "pending");
    assert_eq!(read(&rig, "verb", "freeze"), false.into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(
        read(&rig, "verb", "wet"),
        1.0.into(),
        "as the reverb clamped it"
    );
    assert_eq!(read(&rig, "verb", "room_size"), 0.25.into());
    assert_eq!(read(&rig, "verb", "freeze"), true.into());
    assert!(write_refused(&rig, "verb", "freeze", "maybe".into()));
}

fn write_refused(rig: &Rig, id: &str, key: &str, value: ControlValue) -> bool {
    let surface = rig.live.control_surfaces.lock().unwrap()[id].clone();
    surface.set_control(key, value).is_err()
}

#[test]
fn a_scheduler_ramps_a_reverb_without_allocating() {
    let mut rig = Rig::new(REVERB);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "verb", "control": "room_size", "value": 1.0, "ramp_steps": 4 },
            { "at_step": 0, "module": "verb", "control": "wet", "value": 0.0, "ramp_steps": 4 }
        ]}),
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let ControlValue::Number(room) = read(&rig, "verb", "room_size") else {
        panic!("a number");
    };
    assert!(room > 0.5 && room < 1.0, "{room}");
}

#[test]
fn a_reverb_block_renders_while_the_control_locks_are_held() {
    let mut rig = Rig::new(REVERB);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "verb", "control": "decay", "value": 1.0, "ramp_steps": 4 }
        ]}),
    );
    write(&rig, "verb", "damping", 0.9.into());
    rig.live.write_input("sched", "clock", 1.0).unwrap();

    assert_eq!(counted_block_with_control_locks_held(&mut rig), (0, 0));
    assert_eq!(read(&rig, "verb", "damping"), 0.9.into());
    let ControlValue::Number(decay) = read(&rig, "verb", "decay") else {
        panic!("a number");
    };
    assert!(decay > 0.5, "{decay}");
}
