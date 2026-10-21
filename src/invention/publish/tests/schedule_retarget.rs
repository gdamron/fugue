//! A live `schedule` write that changes what a top-level scheduler writes
//! publishes the topology recompiled with the new targets just ahead of it,
//! so it plays as the same schedule loaded with it would (FUG-388). A write
//! that leaves the targets as compiled publishes nothing.

use super::migrated::{block_while_held, write};
use super::request_beats::render_counted;
use super::schedule_payload::scheduler;
use super::*;
use crate::control_request::CURSOR_CAPACITY;
use crate::invention::publish::timed::EditTime;
use crate::test_support::dial::{DialFactory, DIAL};

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

/// An entry at `step` writing `value` to `module`'s `control`.
fn at(step: u32, module: &str, control: &str, value: f32) -> serde_json::Value {
    serde_json::json!({ "at_step": step, "module": module, "control": control, "value": value })
}

/// The tempo change the scheduler is retargeted onto.
fn tempo() -> serde_json::Value {
    serde_json::json!([at(2, "clock", "bpm", 211.0)])
}

/// A clock at 97 bpm stepping `fillers` empty schedulers and then `sched`,
/// playing `schedule`; the clock's beat and a dial (level 0.25) reach the
/// dac. Past the drain's room, `sched` plays its schedule itself.
fn rig(fillers: usize, schedule: serde_json::Value) -> Rig {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 97.0 }));
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    let empty = serde_json::json!({ "schedule": [] });
    let fillers: Vec<_> = (0..fillers)
        .map(|i| {
            let id = format!("filler{i}");
            let built = rig.build(&id, "control_scheduler", empty.clone());
            (id, built)
        })
        .collect();
    let config = serde_json::json!({ "schedule": schedule });
    let sched = rig.build("sched", "control_scheduler", config);
    rig.live
        .edit(|change| {
            change.upsert("clock", clock);
            change.upsert("dial", dial);
            for (id, filler) in fillers {
                change.upsert(&id, filler);
                change.connect(edge("clock", "beat", &id, "clock"))?;
            }
            change.upsert("sched", sched);
            change.connect(edge("clock", "beat", "sched", "clock"))?;
            change.connect(edge("clock", "beat", "dac", "audio"))?;
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig
}

/// [`rig`] loaded writing the dial, then retargeted live onto the clock's
/// tempo, before it plays.
fn retargeted(fillers: usize) -> Rig {
    let rig = rig(fillers, serde_json::json!([at(2, "dial", "level", 0.5)]));
    write(&rig, "sched", "schedule", tempo().to_string().into());
    rig
}

/// The first sample at which `a` and `b` differ.
fn first_change(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(a, b)| a.to_bits() != b.to_bits())
}

/// Blocks of 64 through about eight beats at 97 bpm.
const BLOCKS: usize = 8 * 48_000 * 60 / 97 / 64;

#[test]
fn a_drain_played_retarget_onto_its_clock_plays_as_loaded() {
    let loaded = rig(0, tempo()).render(BLOCKS);
    let live = retargeted(0).render(BLOCKS);
    assert_eq!(first_change(&live, &loaded), None);
}

#[test]
fn a_self_played_retarget_onto_its_clock_plays_as_loaded() {
    let fillers = CURSOR_CAPACITY;
    let loaded = rig(fillers, tempo()).render(BLOCKS);
    let live = retargeted(fillers).render(BLOCKS);
    assert_eq!(first_change(&live, &loaded), None);
}

/// Orders `source` before `target` (see `change::order`), returning
/// whether it added the order.
fn order(rig: &Rig, source: &str, target: &str) -> bool {
    let mut added = false;
    rig.live
        .edit(|change| {
            added = change.order(source, target);
            Ok(())
        })
        .unwrap();
    added
}

/// The targets the installed topology was compiled with for `id`.
fn compiled_targets(rig: &Rig, id: &str) -> Vec<String> {
    let publisher = rig.live.publisher().lock().unwrap();
    publisher.mirror().targets[id].clone()
}

#[test]
fn a_retarget_publishes_the_topology_recompiled_with_its_targets() {
    let rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    let (generation, modules) = (rig.live.generation(), rig.module_ids());
    assert_eq!(compiled_targets(&rig, "sched"), ["dial"]);
    write(&rig, "sched", "schedule", tempo().to_string().into());
    assert_eq!(rig.live.generation(), generation + 1, "one recompile");
    assert_eq!(compiled_targets(&rig, "sched"), ["clock"]);
    assert_eq!(rig.module_ids(), modules, "nothing built or removed");
    // Installed as a compile now makes it, so an order is judged alone.
    assert!(order(&rig, "clock", "dial"));
}

#[test]
fn a_write_keeping_its_targets_publishes_nothing() {
    let rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    let generation = rig.live.generation();
    let schedule = serde_json::json!([at(1, "dial", "level", 0.75)]);
    write(&rig, "sched", "schedule", schedule.to_string().into());
    assert_eq!(rig.live.generation(), generation);
    // Nor does writing back targets the installed topology has.
    write(&rig, "sched", "schedule", tempo().to_string().into());
    write(&rig, "sched", "schedule", tempo().to_string().into());
    assert_eq!(rig.live.generation(), generation + 1);
}

#[test]
fn a_retarget_while_a_timed_edit_waits_leaves_it_waiting() {
    // Publishing would cancel the edit, which touches nothing the write
    // does: the targets compile with the next change instead, and until
    // then no order publishes (see `change::order`).
    let rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    let mut change = rig.live.begin();
    change.upsert("d2", rig.build("d2", DIAL, serde_json::json!({})));
    let prepared = change.prepare().unwrap();
    let far = EditTime::AtSample(48_000 * 60);
    rig.live.commit_when(prepared, far, None, |_| {}).unwrap();
    let generation = rig.live.generation();
    write(&rig, "sched", "schedule", tempo().to_string().into());
    assert_eq!(rig.live.generation(), generation, "nothing published");
    assert!(
        !order(&rig, "clock", "dial"),
        "compiled with the old targets"
    );
    assert_eq!(rig.live.generation(), generation);
    assert!(rig.live.publisher().lock().unwrap().timed_waiting());
}

#[test]
fn a_recompile_installs_with_no_lock_taken_and_nothing_allocated() {
    let mut rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    render_counted(&mut rig, 4);
    write(&rig, "sched", "schedule", tempo().to_string().into());
    let controls = scheduler(&rig);
    // Every lock a control thread takes is held while it installs.
    let (allocs, frees) = block_while_held(&mut rig, || controls.held()).unwrap();
    assert_eq!((allocs, frees), (0, 0));
    render_counted(&mut rig, BLOCKS);
}

/// The schedule travels in the recompile's publication: with the pending
/// store full, so a request behind the edit would wait, the block that
/// installs the recompile adopts it.
#[test]
fn the_block_installing_a_recompile_adopts_its_schedule() {
    use super::requests::submit;
    use crate::control_request::When;
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let mut rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    render_counted(&mut rig, 1);
    let far = rig.graph.current_sample + 1_000_000;
    let mut next = 0;
    for _ in 0..PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
        for _ in 0..REQUEST_QUEUE_CAPACITY {
            submit(&rig, "dial", 0, 0.25, When::AtSample(far + next));
            next += 1;
        }
        render_counted(&mut rig, 1);
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
    let generation = rig.live.generation();
    write(&rig, "sched", "schedule", tempo().to_string().into());
    assert_eq!(rig.live.generation(), generation + 1);
    let controls = scheduler(&rig);
    assert_ne!(controls.adopted(), controls.recorded(), "not yet installed");
    render_counted(&mut rig, 1);
    assert_eq!(controls.adopted(), controls.recorded());
}
