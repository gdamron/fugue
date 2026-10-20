//! A top-level scheduler steps on one clock's gate output: one whose
//! `clock` input has any other source, or several, is refused, at load
//! and by an offline edit (through a render's controller or the render
//! itself). An unconnected `clock` input is left alone, and
//! so is a scheduler inside a development, which counts any gate's rising
//! edges itself.

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
            { "id": "dac", "type": "dac" }
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
fn a_scheduler_steps_on_a_clock_gate_or_on_nothing() {
    load(serde_json::json!([])).unwrap();
    load(serde_json::json!([to_sched("clock", "beat")])).unwrap();
    load(serde_json::json!([to_sched("clock", "beat_x4")])).unwrap();
}

#[test]
fn a_scheduler_on_another_signal_is_refused_at_load() {
    let error = load(serde_json::json!([to_sched("lfo", "unipolar")])).unwrap_err();
    assert!(
        error.contains("'sched'") && error.contains("'lfo.unipolar'"),
        "{error}"
    );
}

#[test]
fn a_scheduler_on_two_clocks_is_refused_at_load() {
    let both = serde_json::json!([to_sched("clock", "beat"), to_sched("other", "beat")]);
    let error = load(both).unwrap_err();
    assert!(error.contains("'clock.beat' and 'other.beat'"), "{error}");
}

#[test]
fn an_offline_edit_onto_another_signal_is_refused() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_invention(document(serde_json::json!([])))
        .unwrap();
    let controller = engine.controller().unwrap();
    let error = controller
        .connect("lfo", "unipolar", "sched", "clock")
        .unwrap_err();
    assert!(error.to_string().contains("'lfo.unipolar'"), "{error}");
    assert!(
        engine.snapshot().list_connections().is_empty(),
        "nothing connected"
    );
    controller
        .connect("clock", "beat", "sched", "clock")
        .unwrap();
    let error = controller
        .connect("other", "beat", "sched", "clock")
        .unwrap_err();
    assert!(error.to_string().contains("'other.beat'"), "{error}");
}

#[test]
fn a_render_engine_edit_onto_another_signal_is_refused() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_invention(document(serde_json::json!([])))
        .unwrap();
    let error = engine
        .connect("lfo", "unipolar", "sched", "clock")
        .unwrap_err();
    assert!(error.to_string().contains("'lfo.unipolar'"), "{error}");
    assert!(engine.list_connections().is_empty(), "nothing connected");
    engine.connect("clock", "beat", "sched", "clock").unwrap();
    let error = engine
        .connect("other", "beat", "sched", "clock")
        .unwrap_err();
    assert!(error.to_string().contains("'other.beat'"), "{error}");
    assert_eq!(engine.list_connections().len(), 1);
}

/// Replacing a scheduler's clock with a module without its gate output
/// leaves the scheduler unclocked, as a live edit does (which drops the
/// edge): accepted, and an edit afterwards still goes through.
#[test]
fn replacing_a_schedulers_clock_leaves_it_unclocked() {
    let lfo = serde_json::json!({ "rate": 2.0, "waveform": "square" });
    let feeding = serde_json::json!([to_sched("clock", "beat")]);
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(document(feeding.clone())).unwrap();
    engine.add_module("clock", "lfo", &lfo).unwrap();
    engine
        .connect("lfo", "unipolar", "dac", "audio_left")
        .unwrap();
    let mut out = vec![0.0f32; 1024];
    engine.render_interleaved(&mut out).unwrap();
    // The same through the render's controller.
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(document(feeding)).unwrap();
    let controller = engine.controller().unwrap();
    controller.add_module("clock", "lfo", &lfo).unwrap();
    controller
        .connect("lfo", "unipolar", "dac", "audio_left")
        .unwrap();
    // A clock again on the edge left behind is a clock's gate once more.
    let clock = serde_json::json!({ "bpm": 120.0 });
    controller.add_module("clock", "clock", &clock).unwrap();
    let error = controller
        .connect("other", "beat", "sched", "clock")
        .unwrap_err();
    assert!(
        error.to_string().contains("'clock.beat' and 'other.beat'"),
        "{error}"
    );
}

#[test]
fn a_scheduler_connected_twice_to_one_clock_is_told_so() {
    let twice = serde_json::json!([to_sched("clock", "beat"), to_sched("clock", "beat")]);
    let error = load(twice).unwrap_err();
    assert!(error.contains("'clock.beat' is connected twice"), "{error}");
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
