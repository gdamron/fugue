//! A control scheduler's `schedule` on a live graph is a payload: a new
//! schedule is resolved on the control thread and adopted by the audio
//! thread without allocating, freeing or waiting on a lock, at the top
//! level and inside a development. The schedule it replaces is retired:
//! process_block runs in an audio-thread scope, where dropping it panics
//! in debug builds.

use super::migrated::{block_while_held, read, write};
use super::requests::counted_block;
use super::*;
use crate::modules::ControlSchedulerControls;
use crate::ControlValue;

const TOP: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 0, "module": "o", "control": "frequency", "value": 220.0 }
        ] } },
        { "id": "o", "type": "oscillator", "config": { "frequency": 440.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "o", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// `TOP`'s scheduler and oscillator inside a development `pattern`, with
/// the scheduler's clock and `schedule` exposed.
const INNER: &str = r#"{
    "version": "1.0.0",
    "developments": [{ "name": "pattern", "definition": {
        "version": "1.0.0",
        "modules": [
            { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
                { "at_step": 0, "module": "o", "control": "frequency", "value": 220.0 }
            ] } },
            { "id": "o", "type": "oscillator", "config": { "frequency": 440.0 } }
        ],
        "connections": [],
        "inputs": [{ "name": "clock", "to": "sched", "to_port": "clock" }],
        "outputs": [{ "name": "audio", "from": "o", "from_port": "audio" }],
        "controls": [
            { "name": "schedule", "module": "sched", "control": "schedule" },
            { "name": "freq", "module": "o", "control": "frequency" }
        ]
    } }],
    "modules": [
        { "id": "p", "type": "pattern" },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "p", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

const NEXT: &str = r#"[{ "at_step": 1, "module": "o", "control": "frequency", "value": 330.0 }]"#;

/// One block with `id`'s clock input at `level`, counted.
pub(super) fn clocked(rig: &mut Rig, id: &str, level: f32) -> (usize, usize) {
    rig.live.write_input(id, "clock", level).unwrap();
    counted_block(rig)
}

pub(super) fn scheduler(rig: &Rig) -> ControlSchedulerControls {
    let surface = rig.live.control_surfaces.lock().unwrap()["sched"].clone();
    let controls = surface.as_any().unwrap();
    controls
        .downcast_ref::<ControlSchedulerControls>()
        .unwrap()
        .clone()
}

/// Writes `NEXT` to `id`'s `schedule` and checks it is adopted by a block that
/// waits on no lock while every control-side lock (and whatever `hold`
/// takes) is held, then fires on the next step, every block allocation-
/// and free-free; the replaced schedule is freed by the reclaimer.
fn adopted_live<G>(
    rig: &mut Rig,
    id: &str,
    (target, freq): (&str, &str),
    hold: impl FnOnce() -> G + Send,
) {
    assert_eq!(clocked(rig, id, 1.0), (0, 0), "step 0");
    // A development orders its inner modules by signal flow alone, so its
    // scheduler's write may land a block later.
    assert_eq!(counted_block(rig), (0, 0));
    assert_eq!(read(rig, target, freq), ControlValue::Number(220.0));
    write(rig, id, "schedule", NEXT.into());
    rig.live.write_input(id, "clock", 0.0).unwrap();
    assert_eq!(
        block_while_held(rig, hold),
        Ok((0, 0)),
        "the adopting block"
    );
    assert_eq!(clocked(rig, id, 1.0), (0, 0), "step 1");
    assert_eq!(counted_block(rig), (0, 0));
    assert_eq!(read(rig, target, freq), ControlValue::Number(330.0));
    let (_, _, frees) = crate::alloc_counter::allocator_events(|| rig.live.reclaim());
    assert!(frees >= 1, "the reclaimer frees the replaced schedule");
}

#[test]
fn a_live_schedule_is_adopted_without_allocating_or_waiting() {
    let mut rig = Rig::new(TOP);
    rig.render(1);
    let controls = scheduler(&rig);
    adopted_live(&mut rig, "sched", ("o", "frequency"), || controls.held());
    let mut rig = Rig::new(TOP);
    rig.render(1);
    // The scheduler's own reads report the schedule written.
    write(&rig, "sched", "schedule", NEXT.into());
    let ControlValue::String(shown) = read(&rig, "sched", "schedule") else {
        panic!("text");
    };
    assert!(shown.contains("330"), "{shown}");
}

#[test]
fn a_schedule_set_through_a_development_is_adopted_without_allocating_or_waiting() {
    let mut rig = Rig::new(INNER);
    rig.render(1);
    adopted_live(&mut rig, "p", ("p", "freq"), || ());
    let ControlValue::String(shown) = read(&rig, "p", "schedule") else {
        panic!("text");
    };
    assert!(shown.contains("330"), "{shown}");
}

#[test]
fn a_schedule_naming_a_missing_target_is_refused_with_nothing_changed() {
    let mut rig = Rig::new(TOP);
    rig.render(1);
    let surface = rig.live.control_surfaces.lock().unwrap()["sched"].clone();
    let missing = r#"[{ "at_step": 1, "module": "gone", "control": "frequency", "value": 1.0 }]"#;
    assert!(surface.set_control("schedule", missing.into()).is_err());
    assert_eq!(clocked(&mut rig, "sched", 1.0), (0, 0));
    assert_eq!(read(&rig, "o", "frequency"), ControlValue::Number(220.0));
    let ControlValue::String(shown) = read(&rig, "sched", "schedule") else {
        panic!("text");
    };
    assert!(shown.contains("220"), "{shown}");
}

/// A schedule write resolved against the directory before a change
/// published is refused as stale, recording nothing, so the scheduler
/// never adopts targets the change replaced; the setter resolves again.
#[test]
fn a_schedule_resolved_before_a_change_published_is_refused_as_stale() {
    use crate::invention::declared::{Checked, STALE};
    let mut rig = Rig::new(TOP);
    rig.render(1);
    let controls = scheduler(&rig);
    let generation = controls.declared().generation();
    assert!(generation.is_some(), "live");
    let config = serde_json::json!({ "frequency": 440.0 });
    rig.live
        .swap_module(SAMPLE_RATE, "o", "oscillator", &config, true)
        .unwrap();
    let recorded = std::cell::Cell::new(false);
    let checked = Checked {
        generation,
        targets: Vec::new(),
        record: Box::new(|| recorded.set(true)),
    };
    let payload = crate::payload::Payload::new(0_u32);
    let written =
        controls
            .declared()
            .write_payload_checked("schedule", payload, "[]".into(), checked);
    assert_eq!(written, Err(STALE.to_string()));
    assert!(!recorded.get());
    // Written afresh, it resolves against the module the change built.
    write(&rig, "sched", "schedule", NEXT.into());
    assert_eq!(clocked(&mut rig, "sched", 1.0), (0, 0), "step 0");
    assert_eq!(clocked(&mut rig, "sched", 0.0), (0, 0));
    assert_eq!(clocked(&mut rig, "sched", 1.0), (0, 0), "step 1");
    assert_eq!(read(&rig, "o", "frequency"), ControlValue::Number(330.0));
}
