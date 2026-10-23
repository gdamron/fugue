//! A top-level scheduler steps on whatever its `clock` input carries: one
//! clock's gate output (played on the clock's beats), nothing, or any other
//! signal or mix of signals, whose rising edges it counts (FUG-389). So it
//! loads on any of them, and an offline edit (through a render's
//! controller or the render itself) may connect any of them; so may a
//! development's inner graph.

use fugue::{Invention, InventionBuilder, RenderEngine};

/// A clock, an LFO and a scheduler, with `connections` added.
fn document(connections: serde_json::Value) -> Invention {
    let doc = serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "other", "type": "clock", "config": { "bpm": 90.0 } },
            { "id": "lfo", "type": "lfo", "config": { "rate": 2.0, "waveform": "square" } },
            { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
                { "at_step": 1, "module": "clock", "control": "bpm", "value": 100.0 }
            ] } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": connections
    });
    Invention::from_json(&doc.to_string()).unwrap()
}

fn to_sched(from: &str, port: &str) -> serde_json::Value {
    serde_json::json!({ "from": from, "from_port": port, "to": "sched", "to_port": "clock" })
}

fn load(connections: serde_json::Value) -> Result<(), String> {
    let builder = InventionBuilder::new(48_000);
    builder
        .build(document(connections))
        .map(drop)
        .map_err(|error| error.to_string())
}

#[test]
fn a_scheduler_steps_on_a_clock_gate_on_nothing_or_on_any_signal() {
    load(serde_json::json!([])).unwrap();
    load(serde_json::json!([to_sched("clock", "beat")])).unwrap();
    load(serde_json::json!([to_sched("clock", "beat_x4")])).unwrap();
    load(serde_json::json!([to_sched("lfo", "unipolar")])).unwrap();
    let both = serde_json::json!([to_sched("clock", "beat"), to_sched("other", "beat")]);
    load(both).unwrap();
    let twice = serde_json::json!([to_sched("clock", "beat"), to_sched("clock", "beat")]);
    load(twice).unwrap();
}

/// Its `step` output, heard: an LFO at 2 Hz rises every half second, from
/// its first sample.
#[test]
fn a_scheduler_on_an_lfo_counts_its_rising_edges_offline() {
    let heard = serde_json::json!([
        to_sched("lfo", "unipolar"),
        { "from": "sched", "from_port": "step", "to": "dac", "to_port": "audio_left" }
    ]);
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(document(heard)).unwrap();
    let mut out = vec![0.0f32; 2 * 48_000];
    engine.render_interleaved(&mut out).unwrap();
    let step = |second: f32| out[2 * (second * 48_000.0) as usize];
    assert_eq!(step(0.25), 0.0);
    assert_eq!(step(0.75), 1.0);
}

#[test]
fn an_offline_edit_onto_any_signal_is_accepted() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_invention(document(serde_json::json!([])))
        .unwrap();
    let controller = engine.controller().unwrap();
    controller
        .connect("lfo", "unipolar", "sched", "clock")
        .unwrap();
    controller
        .connect("clock", "beat", "sched", "clock")
        .unwrap();
    assert_eq!(engine.snapshot().list_connections().len(), 2);
    engine.connect("other", "beat", "sched", "clock").unwrap();
    assert_eq!(engine.list_connections().len(), 3);
    let mut out = vec![0.0f32; 1024];
    engine.render_interleaved(&mut out).unwrap();
}

/// Whether `connections` still has an edge into the scheduler's clock.
fn clocked(connections: &[fugue::RuntimeConnectionInfo]) -> bool {
    connections
        .iter()
        .any(|conn| conn.to == "sched" && conn.to_port == "clock")
}

/// Replacing a scheduler's clock with a module without its gate output
/// leaves the scheduler unclocked, as a live edit does: the edge through
/// the port the new module lacks is dropped, and a clock put back under the
/// same id does not bring it back. While the schedule still targets the
/// clock, the replacement is refused with nothing changed.
#[test]
fn replacing_a_schedulers_clock_leaves_it_unclocked() {
    let lfo = serde_json::json!({ "rate": 2.0, "waveform": "square" });
    let clock = serde_json::json!({ "bpm": 120.0 });
    let feeding = serde_json::json!([to_sched("clock", "beat")]);
    let elsewhere = fugue::ControlValue::String(
        serde_json::json!([{ "at_step": 1, "module": "other", "control": "bpm", "value": 100.0 }])
            .to_string(),
    );
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(document(feeding.clone())).unwrap();
    assert!(
        engine.add_module("clock", "lfo", &lfo).is_err(),
        "scheduled"
    );
    assert!(clocked(&engine.list_connections()));
    engine
        .set_control("sched", "schedule", elsewhere.clone())
        .unwrap();
    engine.add_module("clock", "lfo", &lfo).unwrap();
    assert!(!clocked(&engine.list_connections()));
    engine.add_module("clock", "clock", &clock).unwrap();
    assert!(!clocked(&engine.list_connections()), "not brought back");
    let mut out = vec![0.0f32; 1024];
    engine.render_interleaved(&mut out).unwrap();
    // The same through the render's controller.
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(document(feeding)).unwrap();
    let controller = engine.controller().unwrap();
    assert!(controller.add_module("clock", "lfo", &lfo).is_err());
    engine.set_control("sched", "schedule", elsewhere).unwrap();
    controller.add_module("clock", "lfo", &lfo).unwrap();
    controller
        .connect("lfo", "unipolar", "dac", "audio_left")
        .unwrap();
    controller.add_module("clock", "clock", &clock).unwrap();
    assert!(!clocked(&engine.list_connections()), "not brought back");
    controller
        .connect("other", "beat", "sched", "clock")
        .unwrap();
    engine.render_interleaved(&mut out).unwrap();
}

#[test]
fn a_scheduler_inside_a_development_steps_on_any_gate() {
    let doc = serde_json::json!({
        "version": "1.0.0",
        "developments": [{ "name": "pulse", "definition": {
            "version": "1.0.0",
            "modules": [
                { "id": "lfo", "type": "lfo", "config": { "rate": 2.0, "waveform": "square" } },
                { "id": "osc", "type": "oscillator", "config": {} },
                { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
                    { "at_step": 1, "module": "osc", "control": "frequency", "value": 330.0 }
                ] } }
            ],
            "connections": [
                { "from": "lfo", "from_port": "unipolar", "to": "sched", "to_port": "clock" }
            ]
        } }],
        "modules": [{ "id": "p", "type": "pulse" }, { "id": "dac", "type": "dac" }],
        "connections": []
    });
    let doc = Invention::from_json(&doc.to_string()).unwrap();
    InventionBuilder::new(48_000).build(doc).unwrap();
}
