//! Room for the ramps of the tables a live graph's drain plays, sized from
//! those tables: a schedule running more ramps at once than the drain's
//! least room glides each as it would alone, whether the graph starts
//! with it, an edit adds it or a live write replaces a schedule with it,
//! on a clock or on edges, live and offline; past the limit, a ramp jumps
//! and the status counts it. No block allocates or frees.

use super::request_beats::render_counted;
use super::*;
use crate::control_request::{RAMP_CAPACITY, TABLE_RAMP_LIMIT};
use crate::invention::declared::bind_offline;
use crate::invention::graph::offline_requests;
use crate::test_support::dial::{DialFactory, DIAL};

/// A tempo with fractional samples per beat at 48 kHz.
const BPM: f64 = 481.0;

/// Blocks of 64 rendered: past every ramp's end, on a clock or on edges.
const BLOCKS: usize = 560;

/// What steps the scheduler: a clock's beat, or an LFO's square's edges.
#[derive(Clone, Copy)]
enum Steps {
    Clock,
    Edges,
}

/// An entry ramping dial `dial`'s level from step 1 over 4 steps: each to
/// its own value, the last dial's to 0.9.
fn ramp(dial: usize, dials: usize) -> serde_json::Value {
    let value = if dial + 1 == dials {
        0.9
    } else {
        0.3 + (dial % 5) as f32 * 0.1
    };
    serde_json::json!({
        "at_step": 1, "module": format!("dial{dial}"), "control": "level",
        "value": value, "ramp_steps": 4
    })
}

/// A schedule ramping each of `ramped` of `dials` dials at once.
fn schedule(dials: usize, ramped: impl IntoIterator<Item = usize>) -> serde_json::Value {
    ramped.into_iter().map(|dial| ramp(dial, dials)).collect()
}

/// `dials` dials (level 0.25), what steps, a scheduler playing `schedule`
/// and the unclipped dac, as a document's modules and connections: the
/// last dial is heard.
fn document(dials: usize, steps: Steps, schedule: &serde_json::Value) -> serde_json::Value {
    let mut modules: Vec<_> = (0..dials)
        .map(|dial| serde_json::json!({ "id": format!("dial{dial}"), "type": DIAL }))
        .collect();
    let (stepper, port) = match steps {
        Steps::Clock => (
            serde_json::json!({ "id": "steps", "type": "clock", "config": { "bpm": BPM } }),
            "beat",
        ),
        Steps::Edges => (
            serde_json::json!({ "id": "steps", "type": "lfo", "config": { "rate": BPM / 60.0, "waveform": "square" } }),
            "unipolar",
        ),
    };
    modules.extend([
        stepper,
        serde_json::json!({ "id": "sched", "type": "control_scheduler", "config": { "schedule": schedule } }),
        serde_json::json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }),
    ]);
    let heard = format!("dial{}", dials - 1);
    serde_json::json!({
        "version": "1.0.0",
        "modules": modules,
        "connections": [
            { "from": "steps", "from_port": port, "to": "sched", "to_port": "clock" },
            { "from": heard, "from_port": "out", "to": "dac", "to_port": "audio" }
        ]
    })
}

fn runtime(document: &serde_json::Value) -> crate::invention::runtime::InventionRuntime {
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let document = Invention::from_json(&document.to_string()).unwrap();
    let builder = InventionBuilder::with_registry(SAMPLE_RATE, registry);
    builder.build(document).unwrap().0
}

/// A live graph starting with [`document`].
fn linked(dials: usize, steps: Steps, schedule: serde_json::Value) -> Rig {
    let runtime = runtime(&document(dials, steps, &schedule));
    let ports = Arc::new(Mutex::new(module_ports(&runtime.modules)));
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    graph.recompile();
    let live = LiveGraph::link(
        &mut graph,
        runtime.state,
        runtime.control_surfaces,
        ports,
        Arc::new(runtime.registry.clone()),
        None,
    );
    Rig {
        graph,
        live,
        registry: runtime.registry,
    }
}

/// A live graph of the dac alone, to which an edit adds the rest of
/// [`document`].
fn edited(dials: usize, schedule: serde_json::Value) -> Rig {
    let empty = serde_json::json!({ "version": "1.0.0", "modules": [], "connections": [] });
    let mut rig = Rig::new(&empty.to_string());
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let document = document(dials, Steps::Clock, &schedule);
    let built: Vec<_> = (document["modules"].as_array().unwrap().iter())
        .map(|module| {
            let id = module["id"].as_str().unwrap();
            let config = module.get("config").cloned().unwrap_or_default();
            let kind = module["type"].as_str().unwrap();
            (id.to_string(), rig.build(id, kind, config))
        })
        .collect();
    rig.live
        .edit(|change| {
            for (id, module) in built {
                change.upsert(&id, module);
            }
            for connection in document["connections"].as_array().unwrap() {
                let end = |key: &str| connection[key].as_str().unwrap();
                let route = edge(end("from"), end("from_port"), end("to"), end("to_port"));
                change.connect(route)?;
            }
            Ok(())
        })
        .unwrap();
    rig
}

/// [`document`] as an offline render loads it.
fn offline(dials: usize, schedule: serde_json::Value) -> Vec<f32> {
    let runtime = runtime(&document(dials, Steps::Clock, &schedule));
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    offline_requests(&mut graph);
    let graph = Arc::new(Mutex::new(graph));
    bind_offline(&graph, &runtime.control_surfaces);
    let mut graph = graph.lock().unwrap();
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    let mut out = Vec::new();
    for _ in 0..BLOCKS {
        graph.process_block(&mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out
}

/// The last of `dials` dials heard when every dial ramps at once, and
/// when it ramps alone: its twin.
fn crowded_and_alone(
    dials: usize,
    rig: impl Fn(serde_json::Value) -> Rig,
) -> (Rig, Vec<f32>, Vec<f32>) {
    let mut alone = rig(schedule(dials, [dials - 1]));
    let alone = render_counted(&mut alone, BLOCKS);
    let mut crowded = rig(schedule(dials, 0..dials));
    let heard = render_counted(&mut crowded, BLOCKS);
    (crowded, heard, alone)
}

/// The heard dial glides, rather than jumps, from 0.25 to 0.9.
fn glides(heard: &[f32]) {
    let between = heard.iter().filter(|&&level| level > 0.26 && level < 0.89);
    assert!(between.count() > 1000, "a glide, not a jump");
    assert_eq!(heard.last(), Some(&0.9));
}

#[test]
fn a_graph_starting_with_more_ramps_than_the_least_room_glides_each_as_alone() {
    let dials = RAMP_CAPACITY + 1;
    for steps in [Steps::Clock, Steps::Edges] {
        let (rig, heard, alone) = crowded_and_alone(dials, |s| linked(dials, steps, s));
        glides(&alone);
        assert_eq!(heard, alone, "bit-exact with its twin");
        assert_eq!(rig.live.table_ramp_jumps(), 0);
    }
}

#[test]
fn an_edit_adding_more_ramps_than_the_drain_holds_hands_it_the_room() {
    let dials = RAMP_CAPACITY + 1;
    let (rig, heard, alone) = crowded_and_alone(dials, |s| edited(dials, s));
    glides(&alone);
    assert_eq!(heard, alone);
    assert_eq!(rig.live.table_ramp_jumps(), 0);
}

#[test]
fn a_live_schedule_write_needing_more_room_takes_it_with_a_recompile() {
    let dials = RAMP_CAPACITY + 1;
    // The same targets, none ramping: no retarget recompiles for it.
    let still: serde_json::Value = (0..dials)
        .map(|dial| serde_json::json!({ "at_step": 1, "module": format!("dial{dial}"), "control": "level", "value": 0.25 }))
        .collect();
    let run = |written: serde_json::Value| {
        let mut rig = linked(dials, Steps::Clock, still.clone());
        render_counted(&mut rig, 1);
        let published = rig.live.publisher().lock().unwrap().generation();
        super::migrated::write(&rig, "sched", "schedule", written.to_string().into());
        let out = render_counted(&mut rig, BLOCKS);
        (rig, published, out)
    };
    let (_, _, alone) = run(schedule(dials, [dials - 1]));
    let (rig, before, heard) = run(schedule(dials, 0..dials));
    glides(&alone);
    assert_eq!(heard, alone);
    assert_eq!(rig.live.table_ramp_jumps(), 0);
    let after = rig.live.publisher().lock().unwrap().generation();
    assert_eq!(after, before + 1, "the room came with a recompile");
}

#[test]
fn an_offline_render_sizes_its_drain_from_its_tables() {
    let dials = RAMP_CAPACITY + 1;
    let alone = offline(dials, schedule(dials, [dials - 1]));
    glides(&alone);
    assert_eq!(offline(dials, schedule(dials, 0..dials)), alone);
}

#[test]
fn past_the_limit_a_ramp_jumps_and_the_status_counts_it() {
    let dials = TABLE_RAMP_LIMIT + 1;
    let mut rig = linked(dials, Steps::Clock, schedule(dials, 0..dials));
    let heard = render_counted(&mut rig, BLOCKS);
    assert_eq!(rig.live.table_ramp_jumps(), 1, "the last, past the limit");
    let jump = heard.iter().position(|&level| level != 0.25).unwrap();
    assert_eq!(heard[jump], 0.9, "it jumps");
}

/// Entries for `dials` dials at step 1, none ramping.
fn still(dials: impl IntoIterator<Item = usize>) -> serde_json::Value {
    let entry = |dial| {
        let module = format!("dial{dial}");
        serde_json::json!({ "at_step": 1, "module": module, "control": "level", "value": 0.25 })
    };
    dials.into_iter().map(entry).collect()
}

#[test]
fn a_schedule_an_edit_writes_with_a_rebuilt_target_takes_its_room() {
    let dials = RAMP_CAPACITY + 1;
    let run = |written: serde_json::Value| {
        let mut rig = linked(dials, Steps::Clock, still(0..dials));
        render_counted(&mut rig, 1);
        let mut change = rig.live.begin();
        change.upsert("dial0", rig.build("dial0", DIAL, serde_json::json!({})));
        change.reschedule("sched", &written.to_string());
        rig.live.commit(change.prepare().unwrap()).unwrap();
        let heard = render_counted(&mut rig, BLOCKS);
        (rig, heard)
    };
    let (_, alone) = run(schedule(dials, [dials - 1]));
    let (rig, heard) = run(schedule(dials, 0..dials));
    glides(&alone);
    assert_eq!(heard, alone);
    assert_eq!(rig.live.table_ramp_jumps(), 0);
}

#[test]
fn a_change_publishing_after_a_write_that_fit_takes_the_room_both_need() {
    let dials = RAMP_CAPACITY + 1;
    let heard_dial = dials - 1;
    // The heard dial is the new scheduler's alone.
    let mut rig = linked(dials, Steps::Clock, still(0..heard_dial));
    render_counted(&mut rig, 1);
    let mut change = rig.live.begin();
    let config = serde_json::json!({ "schedule": schedule(dials, [heard_dial]) });
    change.upsert("sched2", rig.build("sched2", "control_scheduler", config));
    change
        .connect(edge("steps", "beat", "sched2", "clock"))
        .unwrap();
    let prepared = change.prepare().unwrap();
    // The same targets, all ramping: it fits the drain, so publishes nothing.
    let written = schedule(dials, 0..heard_dial);
    super::migrated::write(&rig, "sched", "schedule", written.to_string().into());
    rig.live.commit(prepared).unwrap();
    // The new scheduler counts its steps from the edit on.
    glides(&render_counted(&mut rig, 2 * BLOCKS));
    assert_eq!(rig.live.table_ramp_jumps(), 0);
}

#[test]
fn a_schedule_an_edit_shrinks_keeps_its_room_until_the_shrink_lands() {
    let dials = RAMP_CAPACITY + 1;
    let heard_dial = dials - 1;
    let mut rig = linked(dials, Steps::Clock, schedule(dials, 0..heard_dial));
    render_counted(&mut rig, 1);
    let mut change = rig.live.begin();
    let config = serde_json::json!({ "schedule": schedule(dials, [heard_dial]) });
    change.upsert("sched2", rig.build("sched2", "control_scheduler", config));
    change
        .connect(edge("steps", "beat", "sched2", "clock"))
        .unwrap();
    change.reschedule("sched", &still(0..heard_dial).to_string());
    // Not packed: no target of it is rebuilt, so the old one plays on
    // until a write replaces it, after the change publishes.
    rig.live.commit(change.prepare().unwrap()).unwrap();
    glides(&render_counted(&mut rig, 2 * BLOCKS));
    assert_eq!(rig.live.table_ramp_jumps(), 0);
}
