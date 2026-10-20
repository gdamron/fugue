//! A ramp timed in beats on a development's control starts where its
//! aliases stand, automation written to them and not yet taken included.

use serde_json::json;

use super::*;
use crate::control_request::{BeatHost, ControlTarget};
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::test_support::dial::DialFactory;

#[test]
fn a_ramp_on_a_development_starts_from_automation_its_alias_has_not_taken() {
    let definition: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": [{ "name": "level", "module": "a", "control": "level" }]
    }))
    .unwrap();
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(definition.clone())
        .unwrap();
    let inner = runtime.control_surfaces.lock().unwrap()["a"].clone();
    let (module, surface) = DevelopmentModule::new("one", runtime, &definition).unwrap();
    let control = module.declared().unwrap().0.resolve("level").unwrap();
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let surfaces = Mutex::new(IndexMap::new());
    let module = GraphModule::Module(Box::new(module));
    add_offline(&graph, &surfaces, "one", module, Some(surface)).unwrap();
    // An inner scheduler wrote the alias this block; the dial has not yet
    // processed to take it.
    inner.automation("level").unwrap().write_number(0.75);
    let target = ControlTarget {
        generation: 0,
        module_idx: 0,
        control,
    };
    let mut graph = graph.lock().unwrap();
    assert_eq!(BeatHost::current(&mut *graph, &target), Some(0.75));
}

/// The graph target of `key` on the development `id` in `graph`.
fn target_of(graph: &SignalGraph, id: &str, key: &str) -> ControlTarget {
    let (module_idx, _, instance) = graph.modules.get_full(id).unwrap();
    let control = instance
        .module()
        .declared()
        .unwrap()
        .0
        .resolve(key)
        .unwrap();
    ControlTarget {
        generation: 0,
        module_idx,
        control,
    }
}

#[test]
fn a_ramp_on_a_nested_development_starts_from_its_leaf_after_inner_automation() {
    // The leaf's own scheduler sets its dial at its first beat; neither
    // development's cells see that.
    let leaf = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "a", "type": "dial" },
            { "id": "clock", "type": "clock", "config": { "bpm": 2812.5 } },
            { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
                { "at_step": 0, "module": "a", "control": "level", "value": 0.75 }
            ] } }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" }
        ],
        "controls": [{ "name": "lvl", "module": "a", "control": "level" }]
    });
    let outer = json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": leaf }],
        "modules": [{ "id": "n", "type": "inner" }],
        "connections": [],
        "controls": [{ "name": "lvl", "module": "n", "control": "lvl" }]
    });
    let factory = super::declared_tests::factory("outer", outer);
    let (graph, _) = super::declared_tests::running_from(factory, json!({}));
    let mut graph = graph.lock().unwrap();
    let target = target_of(&graph, "pair", "lvl");
    assert_eq!(BeatHost::current(&mut *graph, &target), Some(0.25));
    // The leaf takes its scheduler's write as it processes.
    for _ in 0..4 {
        graph.modules["pair"].module_mut().process(64);
    }
    assert_eq!(BeatHost::current(&mut *graph, &target), Some(0.75));
}
