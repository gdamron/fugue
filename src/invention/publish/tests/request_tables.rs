//! Tables played by the drain on a live graph and an offline one: each
//! entry lands on the exact sample its step's beat falls on, a ramp glides
//! to land on its end step, both graphs play the same samples, a user's
//! write holds a declared control against a running ramp until the
//! table's next entry on it, tables are found again as the modules move,
//! and a table moved onto anything but a clock's gate is refused. No block allocates, frees
//! or takes a lock.

use super::migrated::block_with_locks_held;
use super::request_beats::{render_counted, Twin};
use super::*;
use crate::control_request::{
    Automation, Request, RequestValue, RtValue, Source, TableEntry, Timeline,
};
use crate::invention::graph::{offline_requests, RoutingConnection};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};
use crate::test_support::table::{entry, TestTable};
use crate::test_support::tabler::{next_table, TablerFactory, TABLER};

mod order;
mod tempi;

/// A tempo with fractional samples per beat at 48 kHz.
const BPM: f64 = 97.0;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": {} },
        { "id": "clock", "type": "clock", "config": { "bpm": 97.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": []
}"#;

fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    registry.register(TablerFactory);
    registry
}

/// A dial, and a tabler playing `entries` on the dial's level, both built
/// against `registry`; the dial alone is heard.
fn build(
    registry: &ModuleRegistry,
    entries: impl FnOnce(&Automation) -> Vec<TableEntry>,
) -> (change::BuiltModule, change::BuiltModule) {
    let config = serde_json::json!({});
    let dial = GraphChange::build(registry, SAMPLE_RATE, "dial", DIAL, &config).unwrap();
    let level = dial.surface.as_ref().unwrap().automation("level").unwrap();
    next_table(TestTable::new(1, entries(&level)));
    let tabler = GraphChange::build(registry, SAMPLE_RATE, "tabler", TABLER, &config).unwrap();
    (dial, tabler)
}

fn edges(clock_port: &str) -> [RoutingConnection; 2] {
    [
        edge("dial", "out", "dac", "audio"),
        edge("clock", clock_port, "tabler", "clock"),
    ]
}

/// The live rig, its tabler gated on the clock's `clock_port`.
fn live(clock_port: &str, entries: impl FnOnce(&Automation) -> Vec<TableEntry>) -> Rig {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.registry.register(TablerFactory);
    let (dial, tabler) = build(&rig.registry, entries);
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("tabler", tabler);
            for edge in edges(clock_port) {
                change.connect(edge)?;
            }
            Ok(())
        })
        .unwrap();
    rig
}

/// The same graph, offline: no request reaches its drain.
fn offline(clock_port: &str, entries: impl FnOnce(&Automation) -> Vec<TableEntry>) -> SignalGraph {
    let registry = registry();
    let document = Invention::from_json(DOCUMENT).unwrap();
    let builder = InventionBuilder::with_registry(SAMPLE_RATE, registry.clone());
    let (mut runtime, _) = builder.build(document).unwrap();
    let (mut dial, mut tabler) = build(&registry, entries);
    let modules = [("dial", &mut dial), ("tabler", &mut tabler)];
    for (id, built) in modules {
        let instance = built.instance.take().unwrap();
        runtime.modules.insert(id.to_string(), instance);
    }
    runtime.routing.extend(edges(clock_port));
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    offline_requests(&mut graph);
    graph.recompile();
    graph
}

fn render_offline(graph: &mut SignalGraph, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
        graph.process_block(&mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out
}

/// The sample each of beats `0..beats` falls on.
fn beat_samples(beats: u32) -> Vec<usize> {
    let mut twin = Twin::new(BPM);
    let mut samples = vec![0];
    for beat in 1..beats {
        let at = twin.until(|clock| clock.position() >= f64::from(beat));
        samples.push(at as usize);
    }
    samples
}

/// Blocks of 64 through beat `beats`.
fn blocks(beats: f64) -> usize {
    (beats * 48_000.0 * 60.0 / BPM / 64.0) as usize + 2
}

fn plan(level: &Automation) -> Vec<TableEntry> {
    vec![
        entry(0, 0, 0.5, level),
        entry(1, 0, 0.75, level),
        entry(2, 2, 1.0, level),
        entry(5, 0, 0.3, level),
    ]
}

/// Checks `out` plays `plan` on `beats`.
fn assert_plays_plan(out: &[f32], beats: &[usize]) {
    assert!(out[..beats[1]].iter().all(|&level| level == 0.5));
    assert!(out[beats[1]..beats[2]].iter().all(|&level| level == 0.75));
    // From step 2 it glides, rising, and lands on step 4 exactly.
    let glide = &out[beats[2]..beats[4]];
    assert!(out[beats[2] - 1] == 0.75 && glide[0] < 0.751);
    assert!(glide.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(*glide.last().unwrap() < 1.0);
    assert!(out[beats[4]..beats[5]].iter().all(|&level| level == 1.0));
    assert!(out[beats[5]..].iter().all(|&level| level == 0.3));
}

#[test]
fn a_table_plays_each_entry_on_its_step_s_sample_live_and_offline_alike() {
    let beats = beat_samples(7);
    let mut rig = live("beat", plan);
    let heard = render_counted(&mut rig, blocks(6.0));
    assert_plays_plan(&heard, &beats);
    let played = rig.graph.modules["tabler"].module().get_output("played");
    assert_eq!(played, Ok(1.0), "the drain plays it");

    let mut graph = offline("beat", plan);
    let rendered = render_offline(&mut graph, blocks(6.0));
    assert_eq!(heard, rendered, "offline plays the same samples");
}

#[test]
fn a_table_steps_on_the_division_its_clock_output_gates() {
    // beat_x2: a step every half beat.
    let beats = beat_samples(4);
    let mut rig = live("beat_x2", |level| vec![entry(4, 0, 0.5, level)]);
    let heard = render_counted(&mut rig, blocks(3.0));
    assert!(heard[..beats[2]].iter().all(|&level| level == 0.25));
    assert!(heard[beats[2]..].iter().all(|&level| level == 0.5));
}

#[test]
fn a_users_write_holds_a_declared_control_against_a_ramp_until_the_next_entry() {
    let beats = beat_samples(8);
    let mut rig = live("beat", |level| {
        vec![entry(1, 4, 1.0, level), entry(6, 0, 0.1, level)]
    });
    // At beat 2.5, halfway up the ramp, a user's write takes the dial.
    let at = (beats[2] + beats[3]) / 2;
    let before = render_counted(&mut rig, at.div_ceil(64));
    let written = rig.graph.current_sample as usize;
    {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("dial", LEVEL).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.6)));
        request.source = Source::User;
        rig.live.requests.submit(request).unwrap();
    }
    let after = render_counted(&mut rig, blocks(7.0) - before.len() / 64);
    let out: Vec<f32> = before.into_iter().chain(after).collect();
    assert!(out[beats[2]..written].iter().all(|&level| level > 0.25));
    // Held through the rest of the ramp, to the table's next entry.
    assert!(out[written..beats[6]].iter().all(|&level| level == 0.6));
    assert!(out[beats[6]..].iter().all(|&level| level == 0.1));
}

#[test]
fn a_table_plays_on_as_an_edit_moves_its_module_and_clock() {
    let beats = beat_samples(7);
    let mut rig = live("beat", plan);
    let before = render_counted(&mut rig, beats[2] / 64);
    // Removing the oscillator moves every module after it.
    rig.live
        .edit(|change| {
            change.remove("osc");
            Ok(())
        })
        .unwrap();
    let after = render_counted(&mut rig, blocks(6.0) - before.len() / 64);
    let out: Vec<f32> = before.into_iter().chain(after).collect();
    assert_plays_plan(&out, &beats);
}

#[test]
fn a_table_moved_onto_no_clock_output_is_refused_and_plays_on() {
    let beats = beat_samples(3);
    let mut rig = live("beat", |level| {
        vec![entry(1, 0, 0.5, level), entry(2, 0, 0.75, level)]
    });
    rig.render(1);
    let generation = rig.live.generation();
    let refused = rig.live.edit(|change| {
        change.disconnect(edge("clock", "beat", "tabler", "clock"));
        change.connect(edge("dial", "out", "tabler", "clock"))
    });
    assert!(matches!(refused, Err(GraphCommandError::InvalidPort(_))));
    assert_eq!(rig.live.generation(), generation, "nothing published");
    let heard = render_counted(&mut rig, beats[2] / 64 + 2);
    assert_eq!(heard[beats[2] - 64], 0.75, "it plays on its clock");
}

#[test]
fn a_block_playing_a_table_takes_no_lock_a_control_thread_holds() {
    let mut rig = live("beat", plan);
    render_counted(&mut rig, 4);
    block_with_locks_held(&mut rig, &[]).unwrap();
}

#[test]
fn a_table_plays_again_from_step_0_on_a_clock_replacing_its_own() {
    let beats = beat_samples(3);
    let mut rig = live("beat", |level| {
        vec![entry(0, 0, 0.5, level), entry(1, 0, 0.75, level)]
    });
    let before = render_counted(&mut rig, beats[2] / 64);
    assert_eq!(before.last(), Some(&0.75), "played out");
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": BPM }));
    rig.live
        .edit(|change| {
            change.upsert("clock", clock);
            Ok(())
        })
        .unwrap();
    // The new clock's beat 0 is the installing block's first sample.
    let after = render_counted(&mut rig, blocks(2.0));
    assert!(after[..beats[1]].iter().all(|&level| level == 0.5));
    assert!(after[beats[1]..].iter().all(|&level| level == 0.75));
}

#[test]
fn a_table_plays_again_from_step_0_on_a_clock_replacing_its_own_offline() {
    use crate::invention::graph::GraphCommand;
    let beats = beat_samples(3);
    let mut graph = offline("beat", |level| {
        vec![entry(0, 0, 0.5, level), entry(1, 0, 0.75, level)]
    });
    let before = render_offline(&mut graph, beats[2] / 64);
    assert_eq!(before.last(), Some(&0.75), "played out");
    // Offline, the clock removed is freed at once: the clock replacing it
    // may take its address, and is told apart all the same.
    graph.apply_command(GraphCommand::RemoveModule {
        module_id: "clock".to_string(),
    });
    let config = serde_json::json!({ "bpm": BPM });
    let mut clock =
        GraphChange::build(&registry(), SAMPLE_RATE, "clock", "clock", &config).unwrap();
    graph.apply_command(GraphCommand::AddModule {
        module_id: "clock".to_string(),
        module: clock.instance.take().unwrap(),
    });
    graph.apply_command(GraphCommand::AddConnection {
        from_module: "clock".to_string(),
        from_port: "beat".to_string(),
        to_module: "tabler".to_string(),
        to_port: "clock".to_string(),
    });
    let after = render_offline(&mut graph, blocks(2.0));
    assert!(after[..beats[1]].iter().all(|&level| level == 0.5));
    assert!(after[beats[1]..].iter().all(|&level| level == 0.75));
}

#[test]
fn a_table_replacing_one_takes_its_room_when_the_tables_room_is_full() {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(TablerFactory);
    let tabler = |rig: &Rig, id: usize| {
        next_table(TestTable::new(id, Vec::new()));
        rig.build(&format!("t{id}"), TABLER, serde_json::json!({}))
    };
    // As many tables as the drain's room (32).
    let tablers: Vec<_> = (0..32).map(|id| (id, tabler(&rig, id))).collect();
    rig.live
        .edit(|change| {
            for (id, built) in tablers {
                change.upsert(&format!("t{id}"), built);
                change.connect(edge("clock", "beat", &format!("t{id}"), "clock"))?;
            }
            Ok(())
        })
        .unwrap();
    rig.render(1);
    let played = |rig: &Rig, id: &str| rig.graph.modules[id].module().get_output("played");
    assert_eq!(played(&rig, "t0"), Ok(1.0));
    let replacing = tabler(&rig, 99);
    rig.live
        .edit(|change| {
            change.upsert("t0", replacing);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    assert_eq!(played(&rig, "t0"), Ok(1.0), "the drain plays it");
}
