//! A development exposing declared inner controls: one write fans out to
//! every alias on the audio thread, and the inner surfaces are reached only
//! through the development.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::take_automation;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::test_support::dial::DialFactory;

/// Two dials behind one exposed `level` (and `pulse` on the first), each
/// to its own output.
fn pair() -> DevelopmentFactory {
    let definition = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [
            { "id": "a", "type": "dial" },
            { "id": "b", "type": "dial" }
        ],
        "connections": [],
        "outputs": [
            { "name": "a", "from": "a", "from_port": "out" },
            { "name": "b", "from": "b", "from_port": "out" }
        ],
        "controls": [
            { "key": "level", "module": "a", "control": "level" },
            { "key": "level", "module": "b", "control": "level" },
            { "key": "pulse", "module": "a", "control": "pulse" }
        ]
    }))
    .unwrap();
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    DevelopmentFactory {
        name: "pair".to_string(),
        definition,
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

/// The development built with `config`, running alone in an offline graph.
fn running(config: serde_json::Value) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    let built = pair().build(48_000, &config).unwrap();
    let surface = built.control_surface.unwrap();
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let surfaces = Mutex::new(IndexMap::new());
    add_offline(
        &graph,
        &surfaces,
        "pair",
        built.module,
        Some(surface.clone()),
    );
    (graph, surface)
}

/// Processes one frame and returns the development's two outputs.
fn outputs(graph: &Arc<Mutex<SignalGraph>>) -> (f32, f32) {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    module.process(1);
    (module.output_block(0)[0], module.output_block(1)[0])
}

#[test]
fn a_config_value_reaches_every_alias_once_the_development_runs() {
    let (graph, surface) = running(json!({ "level": 0.5 }));
    assert_eq!(outputs(&graph), (0.5, 0.5));
    assert_eq!(surface.get_control("level").unwrap(), 0.5.into());
}

#[test]
fn one_write_fans_out_to_every_alias_and_reads_back_the_first() {
    let (graph, surface) = running(json!({}));
    assert_eq!(outputs(&graph), (0.25, 0.25), "each as it was built");
    surface.set_control("level", 2.0.into()).unwrap();
    assert_eq!(outputs(&graph), (1.0, 1.0));
    assert_eq!(surface.get_control("level").unwrap(), 1.0.into());
    surface.set_control("pulse", true.into()).unwrap();
    assert_eq!(outputs(&graph), (2.0, 1.0));
}

#[test]
fn automation_fans_out_on_the_audio_thread_without_allocating() {
    let (graph, surface) = running(json!({}));
    let level = surface.automation("level").unwrap();
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    let ((), allocs, frees) = allocator_events(|| {
        level.write_number(0.75);
        take_automation(module);
        module.process(1);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        (module.output_block(0)[0], module.output_block(1)[0]),
        (0.75, 0.75)
    );
}

#[test]
fn inner_surfaces_are_reached_only_through_the_development() {
    let definition = pair().definition;
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(definition.clone())
        .unwrap();
    let inner = runtime.control_surfaces.lock().unwrap()["a"].clone();
    let (module, _) = DevelopmentModule::new("pair", runtime, &definition).unwrap();
    let refused = inner.set_control("level", 0.5.into()).unwrap_err();
    assert!(refused.contains("through its development"), "{refused}");
    drop(module);
}
