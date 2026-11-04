//! A development schedules its inner graph as the main graph schedules its
//! own: routes into one port sum, a loop runs sample by sample with its
//! back-edges a sample late, and the rest runs a block at a time, so the
//! same wiring renders the same samples nested or flat.

use serde_json::{json, Value};

use crate::alloc_counter::allocator_events;
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::RenderEngine;
use crate::{ControlValue, Invention, InventionBuilder};

const RATE: u32 = 48_000;

fn osc(id: &str, frequency: f64, depth: f64) -> Value {
    json!({ "id": id, "type": "oscillator",
            "config": { "waveform": "sine", "frequency": frequency, "frequency_mod_depth": depth } })
}

fn route(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// Inner modules: `a` ↔ `b` cross-FM (a loop) that an inner lfo and the
/// development's `fm` input also modulate, feeding `c`, which modulates
/// itself (a self-loop), then an acyclic filter tail.
fn inner_modules() -> Vec<Value> {
    vec![
        osc("a", 220.0, 60.0),
        osc("b", 110.0, 45.0),
        json!({ "id": "lfo", "type": "lfo", "config": { "rate": 3.0 } }),
        osc("c", 330.0, 25.0),
        json!({ "id": "tail", "type": "filter",
                "config": { "filter_type": "lowpass", "cutoff": 2400.0, "resonance": 0.3 } }),
    ]
}

fn inner_routes() -> Vec<Value> {
    vec![
        route("a", "audio", "b", "frequency_mod"),
        route("b", "audio", "a", "frequency_mod"),
        route("lfo", "bipolar", "a", "frequency_mod"),
        route("b", "audio", "c", "frequency_mod"),
        route("c", "audio", "c", "frequency_mod"),
        route("c", "audio", "tail", "audio"),
    ]
}

fn nested() -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "knot", "definition": {
            "version": "1.0.0",
            "modules": inner_modules(),
            "connections": inner_routes(),
            "inputs": [{ "name": "fm", "to": "b", "to_port": "frequency_mod" }],
            "outputs": [{ "name": "audio", "from": "tail", "from_port": "audio" }]
        } }],
        "modules": [
            osc("src", 5.0, 0.0),
            { "id": "k", "type": "knot" },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            route("src", "audio", "k", "fm"),
            route("k", "audio", "dac", "audio")
        ]
    }))
    .unwrap()
}

fn flat() -> Invention {
    let mut modules = vec![osc("src", 5.0, 0.0)];
    modules.extend(inner_modules());
    modules.push(json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }));
    let mut connections = vec![route("src", "audio", "b", "frequency_mod")];
    connections.extend(inner_routes());
    connections.push(route("tail", "audio", "dac", "audio"));
    serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": modules,
        "connections": connections
    }))
    .unwrap()
}

fn render(doc: Invention, block: usize, frames: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.load_invention(doc).unwrap();
    engine.set_block_size(block);
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
}

#[test]
fn a_loop_fed_from_outside_and_inside_renders_as_flat() {
    for block in [64, 37, 1] {
        let nested = render(nested(), block, 4_800);
        let flat = render(flat(), block, 4_800);
        assert!(
            nested.iter().any(|x| x.abs() > 0.01),
            "block {block}: silent"
        );
        assert_eq!(first_difference(&nested, &flat), None, "block {block}");
    }
}

/// A clock stepping a development whose scheduler writes `schedule`
/// beside a cross-FM loop it does not touch; the loop's `a` and the
/// scheduled `o` sum into the output's mixer port.
fn looped_pattern(schedule: &str) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{ "name": "pattern", "definition": {{
            "version": "1.0.0",
            "modules": [
                {{ "id": "sched", "type": "control_scheduler", "config": {{ "schedule": {schedule} }} }},
                {{ "id": "m", "type": "oscillator", "config": {{ "frequency": 110.0 }} }},
                {{ "id": "a", "type": "oscillator", "config": {{ "frequency": 220.0, "frequency_mod_depth": 30.0 }} }},
                {{ "id": "b", "type": "oscillator", "config": {{ "frequency": 55.0, "frequency_mod_depth": 30.0 }} }},
                {{ "id": "o", "type": "oscillator", "config": {{ "frequency": 440.0 }} }},
                {{ "id": "out", "type": "vca", "config": {{ "level": 0.5 }} }}
            ],
            "connections": [
                {{ "from": "a", "from_port": "audio", "to": "b", "to_port": "frequency_mod" }},
                {{ "from": "b", "from_port": "audio", "to": "a", "to_port": "frequency_mod" }},
                {{ "from": "a", "from_port": "audio", "to": "out", "to_port": "audio" }},
                {{ "from": "o", "from_port": "audio", "to": "out", "to_port": "audio" }}
            ],
            "inputs": [{{ "name": "clock", "to": "sched", "to_port": "clock" }}],
            "outputs": [{{ "name": "audio", "from": "out", "from_port": "audio" }}],
            "controls": [{{ "name": "schedule", "module": "sched", "control": "schedule" }}]
        }} }}],
        "modules": [
            {{ "id": "clock", "type": "clock", "config": {{ "bpm": 97.0 }} }},
            {{ "id": "p", "type": "pattern" }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [
            {{ "from": "clock", "from_port": "beat", "to": "p", "to_port": "clock" }},
            {{ "from": "p", "from_port": "audio", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

const OLD: &str = r#"[{"at_step":6,"module":"m","control":"frequency","value":220.0}]"#;
const NEW: &str = r#"[{"at_step":6,"module":"o","control":"frequency","value":330.0}]"#;

fn render_writing(doc: &str, write: Option<&str>) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(doc).unwrap();
    let mut out = vec![0.0f32; RATE as usize * 2];
    engine.render_interleaved(&mut out).unwrap();
    if let Some(schedule) = write {
        let schedule = ControlValue::String(schedule.into());
        engine.set_control("p", "schedule", schedule).unwrap();
    }
    let mut rest = vec![0.0f32; RATE as usize * 6];
    engine.render_interleaved(&mut rest).unwrap();
    out.extend(rest);
    out
}

#[test]
fn a_retarget_beside_a_loop_plays_as_loaded() {
    let loaded = render_writing(&looped_pattern(NEW), None);
    let written = render_writing(&looped_pattern(OLD), Some(NEW));
    assert_eq!(first_difference(&loaded, &written), None);
    // The write is heard: the scheduled oscillator changes pitch.
    let unwritten = render_writing(&looped_pattern(OLD), None);
    assert!(first_difference(&loaded, &unwritten).is_some());
}

#[test]
fn a_looped_development_runs_live_without_allocating() {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(nested()).unwrap();
    let (_running, pump) = start_manual(runtime);
    pump.render(4);
    for block in 0..8 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "block {block}");
    }
}

#[test]
fn a_live_retarget_beside_a_loop_costs_the_audio_thread_nothing() {
    let doc: Invention = serde_json::from_str(&looped_pattern(OLD)).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc).unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(4);
    running
        .set_control("p", "schedule", ControlValue::String(NEW.into()))
        .unwrap();
    // The block that adopts the reordered groups, and the ones after it.
    for block in 0..4 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "block {block}");
    }
}
