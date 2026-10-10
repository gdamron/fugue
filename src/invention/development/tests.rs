//! A development keeps its internal control directory, so a scheduler inside
//! it can take a new schedule after the development is built.

use serde_json::json;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::invention::runtime::RunningInvention;
use crate::rpc::StructuralEdit;

/// The `pattern` development: a fast clock gating a scheduler that targets
/// an inner oscillator `o`, exposing the scheduler's `schedule` and the
/// oscillator's `frequency` (as `freq`).
fn pattern() -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 2880.0 } },
            { "id": "sched", "type": "control_scheduler" },
            { "id": "o", "type": "oscillator", "config": { "frequency": 440.0 } }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "gate" }
        ],
        "outputs": [{ "name": "audio", "from": "o", "from_port": "audio" }],
        "controls": [
            { "key": "schedule", "module": "sched", "control": "schedule" },
            { "key": "freq", "module": "o", "control": "frequency" }
        ]
    })
}

/// A document declaring `pattern`, with `modules` (and a dac) at the top.
fn document(modules: serde_json::Value) -> Invention {
    let mut modules = modules.as_array().cloned().unwrap_or_default();
    modules.push(json!({ "id": "dac", "type": "dac" }));
    serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{ "name": "pattern", "definition": pattern() }],
        "modules": modules,
        "connections": []
    }))
    .unwrap()
}

/// A schedule setting `module`'s `frequency` to `value` at step `at`.
fn schedule(at: u32, module: &str, value: f32) -> String {
    format!(r#"[{{ "at": {at}, "module": "{module}", "control": "frequency", "value": {value} }}]"#)
}

fn start(document: Invention) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    start_manual(runtime)
}

fn freq(running: &RunningInvention, module: &str, key: &str) -> ControlValue {
    running.get_control(module, key).unwrap()
}

#[test]
fn a_development_owns_the_directory_its_surfaces_mirror() {
    let definition: Invention = serde_json::from_value(pattern()).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(definition.clone())
        .unwrap();
    let surface =
        DevelopmentControlSurface::new(&definition, runtime.control_surfaces.clone(), None)
            .unwrap();
    drop(runtime);

    // The surface is the directory's only owner, so nothing can change it
    // after build; the lock-free copy it reads and writes through stays
    // the same directory inner setters resolve against.
    assert_eq!(Arc::strong_count(&surface.directory), 1);
    let directory = surface.directory.lock().unwrap();
    assert_eq!(
        directory.keys().collect::<Vec<_>>(),
        surface.surfaces.keys().collect::<Vec<_>>()
    );
    for (id, inner) in directory.iter() {
        assert!(Arc::ptr_eq(inner, &surface.surfaces[id]), "{id}");
    }
}

#[test]
fn an_inner_scheduler_takes_a_schedule_set_through_its_development() {
    let (running, pump) = start(document(json!([{ "id": "lead", "type": "pattern" }])));
    running
        .set_control("lead", "schedule", schedule(0, "o", 220.0).into())
        .expect("the inner scheduler resolves `o` in its own directory");
    pump.render(4);
    assert_eq!(freq(&running, "lead", "freq"), ControlValue::Number(220.0));

    // A later write is adopted live and fires on a later step.
    running
        .set_control("lead", "schedule", schedule(2, "o", 330.0).into())
        .unwrap();
    pump.render(80);
    assert_eq!(freq(&running, "lead", "freq"), ControlValue::Number(330.0));

    // An outer module is not in the development's directory.
    assert!(running
        .set_control("lead", "schedule", schedule(0, "lead", 1.0).into())
        .is_err());
}

#[test]
fn a_development_config_can_set_an_inner_schedule() {
    let config = json!({ "schedule": schedule(0, "o", 220.0) });
    let (running, pump) = start(document(json!([
        { "id": "lead", "type": "pattern", "config": config }
    ])));
    pump.render(4);
    assert_eq!(freq(&running, "lead", "freq"), ControlValue::Number(220.0));

    // One naming an outer module fails the build, as validation does.
    let config = json!({ "schedule": schedule(0, "lead", 220.0) });
    let built = InventionBuilder::new(SAMPLE_RATE).build(document(json!([
        { "id": "lead", "type": "pattern", "config": config }
    ])));
    assert!(built.is_err());
}

#[test]
fn an_added_development_with_a_schedule_starts_without_allocating() {
    let (mut running, pump) = start(document(json!([])));
    pump.render(2);
    running
        .apply_edits(&[StructuralEdit::AddModule {
            id: "lead".into(),
            module_type: "pattern".into(),
            config: json!({ "schedule": schedule(0, "o", 220.0) }),
        }])
        .expect("the batch commits");
    // Publication prepared the inner scheduler: it adopted its schedule off
    // the audio thread, so its first blocks have nothing to allocate.
    for block in 0..2 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "block {block}");
    }
    assert_eq!(freq(&running, "lead", "freq"), ControlValue::Number(220.0));
}

#[test]
fn an_inner_schedule_write_costs_the_audio_thread_what_a_top_level_one_does() {
    // The same live write, to a scheduler inside a development and to one at
    // the top: the block that adopts it pays the same, and later blocks
    // nothing. (Adoption itself allocating is pre-existing for every
    // scheduler; a development adds nothing to it.)
    let mut top: Invention = serde_json::from_value(pattern()).unwrap();
    top.modules.push(crate::ModuleSpec {
        id: "dac".into(),
        module_type: "dac".into(),
        config: json!({}),
    });
    let inner = document(json!([{ "id": "lead", "type": "pattern" }]));
    let mut costs = Vec::new();
    for (document, module) in [(top, "sched"), (inner, "lead")] {
        let (running, pump) = start(document);
        pump.render(2);
        running
            .set_control(module, "schedule", schedule(5, "o", 330.0).into())
            .unwrap();
        let ((), allocs, frees) = allocator_events(|| pump.block());
        let ((), later_allocs, later_frees) = allocator_events(|| pump.block());
        assert_eq!((later_allocs, later_frees), (0, 0), "{module}");
        costs.push((allocs, frees));
    }
    assert_eq!(costs[0], costs[1]);
}
