//! A live `schedule` write that changes what a top-level scheduler writes
//! publishes the topology recompiled with the new targets just ahead of it,
//! so it plays as the same schedule loaded with it would. A write
//! that leaves the targets as compiled publishes nothing.

use super::migrated::{block_while_held, write};
use super::request_beats::render_counted;
use super::requests::submit;
use super::*;
use crate::control_request::When;
use crate::control_request::CURSOR_CAPACITY;
use crate::invention::publish::timed::EditTime;
use crate::modules::ControlSchedulerControls;
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};

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
    rig_at(97.0, fillers, schedule)
}

/// [`rig`] with the clock at `bpm`.
fn rig_at(bpm: f32, fillers: usize, schedule: serde_json::Value) -> Rig {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": bpm }));
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
    let controls = controls_of(&rig, "sched");
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
    let mut rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    render_counted(&mut rig, 1);
    fill_pending(&mut rig, "dial");
    let generation = rig.live.generation();
    write(&rig, "sched", "schedule", tempo().to_string().into());
    assert_eq!(rig.live.generation(), generation + 1);
    let controls = controls_of(&rig, "sched");
    assert_ne!(controls.adopted(), controls.recorded(), "not yet installed");
    render_counted(&mut rig, 1);
    assert_eq!(controls.adopted(), controls.recorded());
}

/// Fills the rig's pending store with far-future writes to `module`'s
/// first control, so a request queued behind them is refused for want of
/// room.
fn fill_pending(rig: &mut Rig, module: &str) {
    use super::requests::submit;
    use crate::control_request::When;
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let far = rig.graph.current_sample + 1_000_000;
    let mut next = 0;
    for _ in 0..PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
        for _ in 0..REQUEST_QUEUE_CAPACITY {
            submit(rig, module, 0, 0.25, When::AtSample(far + next));
            next += 1;
        }
        render_counted(rig, 1);
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
}

/// A scheduler's controls by id.
fn controls_of(rig: &Rig, id: &str) -> ControlSchedulerControls {
    let surface = rig.live.control_surfaces.lock().unwrap()[id].clone();
    let controls = surface.as_any().unwrap();
    controls
        .downcast_ref::<ControlSchedulerControls>()
        .unwrap()
        .clone()
}

/// A recompile orders every scheduler by its last schedule written, so it
/// hands each one that has not adopted its last its schedule too: here
/// one retargeted while a timed edit waited, its request refused.
#[test]
fn a_recompile_hands_a_scheduler_its_unadopted_schedule() {
    let mut rig = rig(0, serde_json::json!([at(2, "dial", "level", 0.5)]));
    let d2 = rig.build("d2", DIAL, serde_json::json!({}));
    let config = serde_json::json!({ "schedule": [at(2, "dial", "level", 0.75)] });
    let sched_b = rig.build("sched_b", "control_scheduler", config);
    rig.live
        .edit(|change| {
            change.upsert("d2", d2);
            change.upsert("sched_b", sched_b);
            change.connect(edge("clock", "beat", "sched_b", "clock"))
        })
        .unwrap();
    render_counted(&mut rig, 1);
    fill_pending(&mut rig, "dial");
    // A timed swap of d2 waits, so retargeting sched recompiles nothing,
    // and its request is refused.
    let mut change = rig.live.begin();
    change.upsert("d2", rig.build("d2", DIAL, serde_json::json!({})));
    let prepared = change.prepare().unwrap();
    let far = EditTime::AtSample(48_000 * 60);
    rig.live.commit_when(prepared, far, None, |_| {}).unwrap();
    write(&rig, "sched", "schedule", tempo().to_string().into());
    render_counted(&mut rig, 1);
    let sched = controls_of(&rig, "sched");
    assert_ne!(sched.adopted(), sched.recorded(), "refused");
    // Retargeted onto d2, sched_b cancels the swap and recompiles.
    let onto_d2 = serde_json::json!([at(2, "d2", "level", 0.75)]);
    write(&rig, "sched_b", "schedule", onto_d2.to_string().into());
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
    assert_eq!(compiled_targets(&rig, "sched"), ["clock"]);
    render_counted(&mut rig, 1);
    let sched_b = controls_of(&rig, "sched_b");
    assert_eq!(sched.adopted(), sched.recorded());
    assert_eq!(sched_b.adopted(), sched_b.recorded());
}

/// A schedule written before the first block plays its step 0 on the
/// install's sample, as a request applied before the beats would, and the
/// one it replaces plays nothing there. (A self-played scheduler
/// adopts it before it processes either way; it is kept as a guard.)
#[test]
fn a_retarget_plays_an_entry_on_the_install_sample_as_loaded() {
    let new = serde_json::json!([at(0, "clock", "bpm", 211.0)]);
    for fillers in [0, CURSOR_CAPACITY] {
        let loaded = rig(fillers, new.clone()).render(BLOCKS);
        let mut live = rig(fillers, serde_json::json!([at(0, "dial", "level", 0.5)]));
        write(&live, "sched", "schedule", new.to_string().into());
        let live = live.render(BLOCKS);
        assert_eq!(first_change(&live, &loaded), None, "{fillers} fillers");
    }
}

/// Renders `rig` up to `sample`, in blocks of 64 and then what is left.
fn render_to(rig: &mut Rig, sample: u64) -> Vec<f32> {
    let blocks = (sample - rig.graph.current_sample) as usize / 64;
    let mut out = render_counted(rig, blocks);
    let rest = (sample - rig.graph.current_sample) as usize;
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    rig.graph
        .process_block(&mut left[..rest], &mut right[..rest]);
    out.extend_from_slice(&left[..rest]);
    out
}

/// A schedule a recompile hands another scheduler replaces its old one
/// before the beats on the install's sample play, as a request applied
/// there would: the step due there plays once, from the new schedule.
#[test]
fn a_handed_schedule_replaces_the_old_on_the_install_sample() {
    let level = |step, value| serde_json::json!([at(step, "dial", "level", value)]);
    let onto_d2 = serde_json::json!([at(4, "d2", "level", 0.75)]);
    let play = |sched: serde_json::Value, sched_b: serde_json::Value| {
        let mut rig = rig(0, sched);
        let d2 = rig.build("d2", DIAL, serde_json::json!({}));
        let config = serde_json::json!({ "schedule": sched_b });
        let sched_b = rig.build("sched_b", "control_scheduler", config);
        rig.live
            .edit(|change| {
                change.upsert("d2", d2);
                change.upsert("sched_b", sched_b);
                change.connect(edge("clock", "beat", "sched_b", "clock"))
            })
            .unwrap();
        render_counted(&mut rig, 1);
        rig
    };
    // Where step 4 falls: the old schedule's entry there.
    let unchanged = play(level(20, 0.5), level(20, 0.25)).render(BLOCKS);
    let old = play(level(4, 0.5), level(20, 0.25)).render(BLOCKS);
    // Each rendered from the second block.
    let step = 64 + first_change(&old, &unchanged).unwrap() as u64;
    let mut loaded = play(level(4, 0.75), onto_d2.clone());
    let mut live = play(level(4, 0.5), level(20, 0.25));
    // sched's write keeps its targets: a request, refused for want of
    // room (writes to d2 fill it, which reach nothing heard); sched_b's
    // retarget then recompiles, handing it over.
    fill_pending(&mut live, "d2");
    write(
        &live,
        "sched",
        "schedule",
        level(4, 0.75).to_string().into(),
    );
    let start = live.graph.current_sample;
    render_to(&mut loaded, start);
    let mut heard = render_to(&mut live, step);
    let sched = controls_of(&live, "sched");
    assert_ne!(sched.adopted(), sched.recorded(), "refused");
    let generation = live.live.generation();
    write(&live, "sched_b", "schedule", onto_d2.to_string().into());
    assert_eq!(live.live.generation(), generation + 1, "recompiled");
    heard.extend(render_counted(&mut live, 750));
    let oracle = render_to(&mut loaded, start + heard.len() as u64);
    assert_eq!(first_change(&heard, &oracle), None);
}

/// Where step 4 falls: where a schedule writing the dial there first
/// differs from one that never does.
fn step_four(fillers: usize) -> u64 {
    let mut quiet = rig(fillers, serde_json::json!([at(20, "dial", "level", 0.5)]));
    let mut old = rig(fillers, serde_json::json!([at(4, "dial", "level", 0.5)]));
    first_change(&old.render(BLOCKS), &quiet.render(BLOCKS)).unwrap() as u64
}

/// Retargeted mid-play so the recompile installs on a step's sample, the
/// step plays once, from the new schedule: the old one's entry never
/// plays, and the new one's does, as loaded.
#[test]
fn a_retarget_installing_on_a_step_plays_it_from_the_new_schedule() {
    let new = serde_json::json!([at(4, "clock", "bpm", 211.0)]);
    for fillers in [0, CURSOR_CAPACITY] {
        let step = step_four(fillers);
        let loaded = rig(fillers, new.clone()).render(BLOCKS);
        let mut live = rig(fillers, serde_json::json!([at(4, "dial", "level", 0.5)]));
        let mut heard = render_to(&mut live, step);
        let generation = live.live.generation();
        write(&live, "sched", "schedule", new.to_string().into());
        assert_eq!(live.live.generation(), generation + 1, "recompiled");
        heard.extend(render_counted(&mut live, BLOCKS - heard.len() / 64));
        assert_eq!(first_change(&heard, &loaded), None, "{fillers} fillers");
    }
}

/// Late writes to a module a recompile keeps, one queued before it and one
/// behind it and due earlier, apply in their samples' order, as with no
/// edit between them: the one due later is heard. The recompile hands the
/// scheduler its schedule with the requests all the same.
#[test]
fn late_writes_on_either_side_of_a_recompile_apply_in_time_order() {
    let play = |recompile: bool| {
        let mut rig = rig(0, serde_json::json!([at(40, "dial", "level", 0.25)]));
        render_counted(&mut rig, 1);
        let start = rig.graph.current_sample;
        submit(&rig, "dial", LEVEL.0, 0.5, When::AtSample(start - 2));
        if recompile {
            let onto_clock = serde_json::json!([at(40, "clock", "bpm", 97.0)]);
            write(&rig, "sched", "schedule", onto_clock.to_string().into());
        }
        submit(&rig, "dial", LEVEL.0, 0.75, When::AtSample(start - 4));
        let heard = render_counted(&mut rig, 4);
        let sched = controls_of(&rig, "sched");
        assert_eq!(sched.adopted(), sched.recorded(), "adopted");
        heard
    };
    let (live, oracle) = (play(true), play(false));
    assert_eq!(first_change(&live, &oracle), None);
}

/// A scheduler at 512 bpm writing the dial 0.5 on its ninth step, inside
/// the block from sample 50_624; at that block's start a write of 0.2 due
/// two samples before it, then `edit`. The scheduler's write, due after
/// the late one, stands: the edit drops its table but not its write.
fn a_dropped_tables_write_stands(edit: fn(&Rig)) {
    let play = |with_edit: bool| {
        let mut rig = rig_at(512.0, 0, serde_json::json!([at(9, "dial", "level", 0.5)]));
        let mut out = render_counted(&mut rig, 50_624 / 64);
        let start = rig.graph.current_sample;
        submit(&rig, "dial", LEVEL.0, 0.2, When::AtSample(start - 2));
        if with_edit {
            edit(&rig);
        }
        out.extend(render_counted(&mut rig, 64));
        out
    };
    let (edited, twin) = (play(true), play(false));
    assert_eq!(edited.last(), Some(&0.5), "the scheduler's write is heard");
    assert_eq!(first_change(&edited, &twin), None);
}

#[test]
fn a_scheduler_an_edit_removes_keeps_its_drained_write() {
    a_dropped_tables_write_stands(|rig| {
        rig.live
            .edit(|change| {
                change.remove("sched");
                Ok(())
            })
            .unwrap();
    });
}

#[test]
fn a_scheduler_an_edit_takes_off_its_clock_keeps_its_drained_write() {
    a_dropped_tables_write_stands(|rig| {
        let config = serde_json::json!({ "schedule": [at(9, "dial", "level", 0.5)] });
        let sched = rig.build("sched", "control_scheduler", config);
        rig.live
            .edit(|change| {
                change.upsert("sched", sched);
                change.disconnect(edge("clock", "beat", "sched", "clock"));
                Ok(())
            })
            .unwrap();
    });
}
