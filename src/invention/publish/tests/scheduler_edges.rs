//! A top-level `control_scheduler` stepping on a gate other than a clock's
//! (an LFO's square), played by the graph's request drain on its rising
//! edges (FUG-389): each entry lands on the sample of its step's edge,
//! whatever the block size, live and offline alike; a ramp lands each
//! boundary on its edge; the `reset` input restarts the schedule; and no
//! block allocates, frees or takes a lock a control thread holds.

use super::migrated::{block_with_locks_held, read, write};
use super::request_beats::render_counted;
use super::scheduler_beats::level;
use super::*;
use crate::invention::declared::bind_offline;
use crate::invention::graph::offline_requests;
use crate::test_support::dial::{DialFactory, DIAL};
use crate::ControlValue;

/// An LFO square's rate: a step every 1291 samples or so, never on a
/// block's edge for long.
const RATE: f64 = 37.17;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

/// The modules of [`rig`], as a document's JSON.
fn modules(schedule: &serde_json::Value) -> [(&'static str, &'static str, serde_json::Value); 4] {
    let square = |rate: f64| serde_json::json!({ "rate": rate, "waveform": "square" });
    [
        ("dial", DIAL, serde_json::json!({})),
        ("lfo", "lfo", square(RATE)),
        ("slow", "lfo", square(RATE / 7.3)),
        (
            "sched",
            "control_scheduler",
            serde_json::json!({ "schedule": schedule }),
        ),
    ]
}

/// A dial (level 0.25) and a scheduler playing `schedule`, stepping on an
/// LFO's square (`reset` on a slower one's, when `reset`); `heard` reaches
/// the unclipped dac.
fn rig(schedule: serde_json::Value, heard: (&str, &str), reset: bool) -> Rig {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let built: Vec<_> = modules(&schedule)
        .into_iter()
        .map(|(id, kind, config)| (id, rig.build(id, kind, config)))
        .collect();
    rig.live
        .edit(|change| {
            for (id, module) in built {
                change.upsert(id, module);
            }
            change.connect(edge("lfo", "unipolar", "sched", "clock"))?;
            if reset {
                change.connect(edge("slow", "unipolar", "sched", "reset"))?;
            }
            change.connect(edge(heard.0, heard.1, "dac", "audio"))
        })
        .unwrap();
    rig
}

/// The same graph, offline, as a render loads it: built from its
/// document, no request reaching its drain.
fn offline(schedule: serde_json::Value, heard: (&str, &str)) -> Arc<Mutex<SignalGraph>> {
    let modules: Vec<_> = modules(&schedule)
        .into_iter()
        .map(|(id, kind, config)| serde_json::json!({ "id": id, "type": kind, "config": config }))
        .chain([
            serde_json::json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }),
        ])
        .collect();
    let document = serde_json::json!({
        "version": "1.0.0",
        "modules": modules,
        "connections": [
            { "from": "lfo", "from_port": "unipolar", "to": "sched", "to_port": "clock" },
            { "from": heard.0, "from_port": heard.1, "to": "dac", "to_port": "audio" }
        ]
    });
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let document = Invention::from_json(&document.to_string()).unwrap();
    let builder = InventionBuilder::with_registry(SAMPLE_RATE, registry);
    let (runtime, _) = builder.build(document).unwrap();
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    offline_requests(&mut graph);
    let graph = Arc::new(Mutex::new(graph));
    bind_offline(&graph, &runtime.control_surfaces);
    graph
}

/// `samples` of `graph`'s left channel, in blocks of `block`.
fn render(graph: &mut SignalGraph, block: usize, samples: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    while out.len() < samples {
        graph.process_block(&mut left[..block], &mut right[..block]);
        out.extend_from_slice(&left[..block]);
    }
    out
}

/// The samples `signal` rises past 0.5 on, from a level of 0.
fn rises(signal: &[f32]) -> Vec<usize> {
    let mut last = 0.0;
    let mut rises = Vec::new();
    for (at, &level) in signal.iter().enumerate() {
        if level > 0.5 && last <= 0.5 {
            rises.push(at);
        }
        last = level;
    }
    rises
}

/// The samples the LFO's square rises on, through `samples`.
fn edges(samples: usize) -> Vec<usize> {
    let mut rig = rig(serde_json::json!([]), ("lfo", "unipolar"), false);
    rises(&render_counted(&mut rig, samples / 64 + 1))
}

/// A ramp from step `step` over `steps` steps to `value`.
fn ramp(step: u32, steps: u32, value: f32) -> serde_json::Value {
    serde_json::json!({
        "at_step": step, "module": "dial", "control": "level", "value": value, "ramp_steps": steps
    })
}

#[test]
fn each_entry_lands_on_its_edge_s_sample_at_any_block_size() {
    let schedule = serde_json::json!([level(1, 0.5), level(3, 0.75), level(6, 0.1)]);
    let edges = edges(64 * 160);
    assert!(edges.len() > 7 && edges.iter().any(|at| at % 64 != 0));
    let mut live = rig(schedule.clone(), ("dial", "out"), false);
    let heard = render_counted(&mut live, 160);
    let span = |from: usize, to: usize| &heard[edges[from]..edges[to]];
    assert!(heard[..edges[1]].iter().all(|&level| level == 0.25));
    assert!(span(1, 3).iter().all(|&level| level == 0.5));
    assert!(span(3, 6).iter().all(|&level| level == 0.75));
    assert!(heard[edges[6]..].iter().all(|&level| level == 0.1));
    for block in [1, 17, 64] {
        let mut blocked = rig(schedule.clone(), ("dial", "out"), false);
        let out = render(&mut blocked.graph, block, heard.len());
        assert_eq!(out[..heard.len()], heard[..], "blocks of {block}");
    }
    let graph = offline(schedule, ("dial", "out"));
    let rendered = render(&mut graph.lock().unwrap(), 64, heard.len());
    assert_eq!(rendered, heard, "offline plays the same samples");
}

#[test]
fn the_step_output_counts_the_edges() {
    let edges = edges(64 * 40);
    let mut rig = rig(serde_json::json!([level(9, 0.5)]), ("sched", "step"), false);
    let out = render_counted(&mut rig, 40);
    let edges: Vec<_> = edges.into_iter().filter(|&at| at < out.len()).collect();
    assert!(out[..edges[0]].iter().all(|&step| step == -1.0));
    for (step, span) in edges.windows(2).enumerate() {
        let span = &out[span[0]..span[1]];
        assert!(span.iter().all(|&at| at == step as f32), "step {step}");
    }
}

#[test]
fn a_ramp_lands_each_boundary_on_its_edge() {
    let schedule = serde_json::json!([ramp(1, 4, 1.0), level(7, 0.1)]);
    let edges = edges(64 * 160);
    let mut rig = rig(schedule, ("dial", "out"), false);
    let out = render_counted(&mut rig, 160);
    assert_eq!(
        out[edges[1]], 0.25,
        "a ramp starts where the control stands"
    );
    for k in 1..4 {
        assert_eq!(
            out[edges[1 + k]],
            0.25 + 0.75 * k as f32 / 4.0,
            "boundary {k}"
        );
    }
    let glide = &out[edges[1]..edges[5]];
    assert!(glide.windows(2).all(|pair| pair[0] <= pair[1]));
    let first = &out[edges[1] + 1..edges[2]];
    let gliding = |level: &f32| *level > 0.25 && *level < 0.4375;
    assert!(first.iter().any(gliding), "it moves on between edges");
    assert!(out[edges[5]..edges[7]].iter().all(|&level| level == 1.0));
    assert!(out[edges[7]..].iter().all(|&level| level == 0.1));
}

#[test]
fn a_rise_on_reset_plays_the_schedule_again_from_step_0() {
    let schedule = serde_json::json!([level(0, 0.5), level(1, 0.75)]);
    let mut slow = rig(serde_json::json!([]), ("slow", "unipolar"), false);
    let resets = rises(&render_counted(&mut slow, 400));
    let edges = edges(64 * 400);
    let mut rig = rig(schedule, ("dial", "out"), true);
    let out = render_counted(&mut rig, 400);
    // The first edge after the second reset steps it to 0 again.
    let reset = resets[1];
    let first = edges.iter().position(|&at| at >= reset).unwrap();
    assert_eq!(out[reset - 1], 0.75);
    assert!(out[reset..edges[first]].iter().all(|&level| level == 0.75));
    assert_eq!(out[edges[first]], 0.5);
    assert_eq!(out[edges[first + 1]], 0.75);
}

/// A rise on `reset` ends the ramp where it stands; step 1 starts it
/// again from there.
#[test]
fn a_rise_on_reset_ends_a_ramp_where_it_stands() {
    let mut slow = rig(serde_json::json!([]), ("slow", "unipolar"), false);
    let reset = rises(&render_counted(&mut slow, 400))[1];
    let edges = edges(64 * 400);
    let mut rig = rig(serde_json::json!([ramp(1, 40, 1.0)]), ("dial", "out"), true);
    let out = render_counted(&mut rig, 400);
    let first = edges.iter().position(|&at| at >= reset).unwrap();
    let stood = out[reset];
    assert!(stood > 0.25 && stood < 1.0, "mid-ramp: {stood}");
    assert!(out[reset..=edges[first]]
        .iter()
        .all(|&level| level == stood));
    assert!(out[edges[first + 1] + 64] > stood, "step 1 starts it again");
}

#[test]
fn a_schedule_written_live_plays_on_from_the_step_counted() {
    let edges = edges(64 * 160);
    let mut live = rig(serde_json::json!([level(1, 0.5)]), ("dial", "out"), false);
    let before = render_counted(&mut live, edges[3] / 64 + 1);
    let schedule = serde_json::json!([level(2, 0.9), level(5, 0.6)]);
    write(&live, "sched", "schedule", schedule.to_string().into());
    let after = render_counted(&mut live, 160 - before.len() / 64);
    let out: Vec<f32> = before.into_iter().chain(after).collect();
    // Step 2 had its edge: only step 5 plays.
    assert!(out[edges[1]..edges[5]].iter().all(|&level| level == 0.5));
    assert!(out[edges[5]..].iter().all(|&level| level == 0.6));
}

/// A target rebuilt by an edit mid-ramp (FUG-402): the scheduler adopts
/// the same plan retargeted, and the ramp plays on into the new module,
/// each boundary on its edge.
#[test]
fn a_ramp_plays_on_into_a_target_an_edit_rebuilds() {
    let edges = edges(64 * 160);
    let mut live = rig(serde_json::json!([ramp(1, 4, 1.0)]), ("dial", "out"), false);
    let before = render_counted(&mut live, edges[3] / 64 + 1);
    let dial = live.build("dial", DIAL, serde_json::json!({}));
    live.live
        .edit(|change| {
            change.upsert("dial", dial);
            Ok(())
        })
        .unwrap();
    let after = render_counted(&mut live, 160 - before.len() / 64);
    let out: Vec<f32> = before.into_iter().chain(after).collect();
    assert_eq!(out[edges[4]], 0.25 + 0.75 * 3.0 / 4.0);
    assert!(out[edges[5]..].iter().all(|&level| level == 1.0));
}

#[test]
fn a_schedule_moved_from_a_gate_to_a_clock_starts_over_on_the_clock() {
    let schedule = serde_json::json!([level(0, 0.5), level(1, 0.75)]);
    let mut rig = rig(schedule, ("dial", "out"), false);
    render_counted(&mut rig, 80);
    assert_eq!(read(&rig, "dial", "level"), ControlValue::Number(0.75));
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 120.0 }));
    rig.live
        .edit(|change| {
            change.upsert("clock", clock);
            change.disconnect(edge("lfo", "unipolar", "sched", "clock"));
            change.connect(edge("clock", "beat", "sched", "clock"))
        })
        .unwrap();
    let out = render_counted(&mut rig, 2);
    assert_eq!(out[0], 0.5, "step 0 on the clock's first beat");
}

#[test]
fn a_block_playing_a_schedule_on_edges_takes_no_lock_a_control_thread_holds() {
    let schedule = serde_json::json!([ramp(0, 8, 1.0), level(9, 0.2)]);
    let mut rig = rig(schedule, ("dial", "out"), true);
    render_counted(&mut rig, 4);
    for _ in 0..8 {
        block_with_locks_held(&mut rig, &[]).unwrap();
    }
}

/// A target the drain cannot split on the edges (it computes in a feedback
/// group) has its entry land late: at the start of the segment after its
/// edge.
#[test]
fn an_entry_on_a_feedback_member_lands_at_the_next_segment() {
    let schedule = serde_json::json!([
        { "at_step": 1, "module": "vca", "control": "level", "value": 0.5 }
    ]);
    let edges = edges(64 * 80);
    let mut rig = rig(serde_json::json!([]), ("dial", "out"), false);
    let vca = rig.build("vca", "vca", serde_json::json!({}));
    let back = rig.build("back", "vca", serde_json::json!({ "level": 0.0 }));
    rig.live
        .edit(|change| {
            change.upsert("vca", vca);
            change.upsert("back", back);
            change.disconnect(edge("dial", "out", "dac", "audio"));
            change.connect(edge("dial", "out", "vca", "audio"))?;
            change.connect(edge("back", "audio", "vca", "audio"))?;
            change.connect(edge("vca", "audio", "back", "audio"))?;
            change.connect(edge("vca", "audio", "dac", "audio"))
        })
        .unwrap();
    write(&rig, "sched", "schedule", schedule.to_string().into());
    let out = render_counted(&mut rig, 80);
    let late = edges[1].next_multiple_of(64);
    assert_ne!(late, edges[1], "the edge falls inside a block");
    assert!(out[..late].iter().all(|&level| level == 0.25));
    assert!(out[late..].iter().all(|&level| level == 0.125));
}

/// Moved from a clock onto a gate, a schedule plays on from the step the
/// clock reached: the gate's next edge is the step after it.
#[test]
fn a_schedule_moved_from_a_clock_onto_a_gate_plays_on_from_the_clock_s_step() {
    let schedule = serde_json::json!([level(0, 0.5), level(2, 0.75), level(3, 0.1)]);
    let mut rig = rig(schedule, ("dial", "out"), false);
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 120.0 }));
    rig.live
        .edit(|change| {
            change.upsert("clock", clock);
            change.disconnect(edge("lfo", "unipolar", "sched", "clock"));
            change.connect(edge("clock", "beat", "sched", "clock"))
        })
        .unwrap();
    // Beat 1 of the clock (24000 samples a beat) is step 1.
    render_counted(&mut rig, 36_000 / 64);
    assert_eq!(read(&rig, "dial", "level"), ControlValue::Number(0.5));
    rig.live
        .edit(|change| {
            change.disconnect(edge("clock", "beat", "sched", "clock"));
            change.connect(edge("lfo", "unipolar", "sched", "clock"))
        })
        .unwrap();
    let out = render_counted(&mut rig, 80);
    let moved = 36_000 / 64 * 64;
    let next: Vec<_> = edges(moved + 64 * 80)
        .into_iter()
        .filter(|&at| at >= moved)
        .map(|at| at - moved)
        .collect();
    let changes: Vec<(usize, f32)> = (1..out.len())
        .filter(|&at| out[at] != out[at - 1])
        .map(|at| (at, out[at]))
        .collect();
    assert_eq!(
        changes,
        [(next[0], 0.75), (next[1], 0.1)],
        "steps 2 and 3 on the gate's next edges"
    );
}
