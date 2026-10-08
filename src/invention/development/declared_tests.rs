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

/// A development factory over `definition`, with dials registered.
fn factory(name: &str, definition: serde_json::Value) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    DevelopmentFactory {
        name: name.to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

/// A dial and a (legacy) lfo behind one exposed `mix`, with the lfo's rate
/// also exposed alone as `rate`, and `odd` reaching a dial's number and
/// its choice at once.
fn mixed() -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }, { "id": "l", "type": "lfo" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": [
            { "key": "mix", "module": "a", "control": "level" },
            { "key": "mix", "module": "l", "control": "rate" },
            { "key": "rate", "module": "l", "control": "rate" },
            { "key": "odd", "module": "a", "control": "level" },
            { "key": "odd", "module": "a", "control": "shape" }
        ]
    })
}

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
    running_from(pair(), config)
}

fn running_from(
    factory: DevelopmentFactory,
    config: serde_json::Value,
) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    let built = factory.build(48_000, &config).unwrap();
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
    )
    .unwrap();
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

#[test]
fn a_key_mixing_declared_and_legacy_aliases_reaches_both_but_cannot_be_scheduled() {
    let (graph, surface) = running_from(factory("mixed", mixed()), json!({ "mix": 3.0 }));
    assert_eq!(outputs_a(&graph), 1.0, "the dial, clamped");
    assert_eq!(surface.get_control("rate").unwrap(), 3.0.into(), "the lfo");
    surface.set_control("mix", 0.5.into()).unwrap();
    assert_eq!(outputs_a(&graph), 0.5);
    assert_eq!(surface.get_control("rate").unwrap(), 0.5.into());
    assert!(surface.declares("mix") && surface.automation("mix").is_none());
}

#[test]
fn a_nested_mixed_key_reaches_both_parts_through_the_outer_development() {
    let outer = json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": mixed() }],
        "modules": [{ "id": "n", "type": "inner" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "n", "from_port": "a" }],
        "controls": [
            { "key": "mix", "module": "n", "control": "mix" },
            { "key": "rate", "module": "n", "control": "rate" }
        ]
    });
    let (graph, surface) = running_from(factory("outer", outer), json!({ "mix": 0.5 }));
    assert_eq!(outputs_a(&graph), 0.5);
    assert_eq!(surface.get_control("rate").unwrap(), 0.5.into());
    surface.set_control("mix", 0.75.into()).unwrap();
    assert_eq!(outputs_a(&graph), 0.75);
    assert_eq!(surface.get_control("rate").unwrap(), 0.75.into());
}

#[test]
fn a_value_one_alias_cannot_hold_changes_no_alias_and_fails_a_cold_load() {
    let (graph, surface) = running_from(factory("mixed", mixed()), json!({}));
    let refused = surface.set_control("odd", 0.5.into()).unwrap_err();
    assert!(refused.contains("string"), "{refused}");
    assert_eq!(outputs_a(&graph), 0.25, "the level alias is untouched too");
    let config = json!({ "odd": 0.5 });
    assert!(factory("mixed", mixed()).build(48_000, &config).is_err());
}

#[test]
fn a_key_with_any_event_alias_is_an_event_in_either_order() {
    for aliases in [["level", "pulse"], ["pulse", "level"]] {
        let definition = json!({
            "version": "1.0.0",
            "modules": [{ "id": "a", "type": "dial" }, { "id": "b", "type": "dial" }],
            "connections": [],
            "controls": [
                { "key": "x", "module": "a", "control": aliases[0] },
                { "key": "x", "module": "b", "control": aliases[1] }
            ]
        });
        let built = factory("events", definition)
            .build(48_000, &json!({}))
            .unwrap();
        let declaration = built.control_surface.unwrap().declaration("x").unwrap();
        assert!(declaration.decl.event, "{aliases:?}");
    }
}

#[test]
fn scheduled_writes_fan_out_unclamped_so_each_alias_clamps_for_itself() {
    let (_, surface) = running(json!({}));
    let level = surface.automation("level").unwrap();
    assert_eq!(level.clamp, None);
}

#[test]
fn a_ramp_starts_from_what_the_first_alias_holds() {
    let definition: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": [{ "key": "level", "module": "a", "control": "level" }]
    }))
    .unwrap();
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(definition.clone())
        .unwrap();
    let inner = runtime.control_surfaces.lock().unwrap()["a"].clone();
    let (mut module, surface) = DevelopmentModule::new("one", runtime, &definition).unwrap();
    // An inner scheduler moves the alias; the development's cells never see it.
    inner.automation("level").unwrap().write_number(0.6);
    module.process(1);
    assert_eq!(surface.automation("level").unwrap().current(), Some(0.6));
}

/// Processes one frame and returns the development's first output.
fn outputs_a(graph: &Arc<Mutex<SignalGraph>>) -> f32 {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    module.process(1);
    module.output_block(0)[0]
}

#[test]
fn a_scheduled_ramp_on_a_legacy_only_key_allocates_nothing() {
    use crate::modules::control_scheduler::{
        ControlScheduler, ControlSchedulerControls, ScheduleEntry, ScheduleValue,
    };
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "l", "type": "lfo" }],
        "connections": [],
        "controls": [{ "key": "rate", "module": "l", "control": "rate" }]
    });
    let built = factory("legacy", definition)
        .build(48_000, &json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    let mut map = IndexMap::new();
    map.insert("dev".to_string(), surface.clone());
    let directory = Arc::new(Mutex::new(map));
    let entry = ScheduleEntry {
        at: 0,
        module: "dev".into(),
        control: "rate".into(),
        value: ScheduleValue::Number(4.0),
        ramp: Some(2),
    };
    let controls = ControlSchedulerControls::new(vec![entry]);
    controls.attach("sched", &directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, controls);
    scheduler.prepare_for_publication();
    for gate in [1.0, 0.0, 1.0, 0.0] {
        scheduler.set_input("gate", gate).unwrap();
        let (_, allocs, frees) = allocator_events(|| scheduler.process(64));
        assert_eq!((allocs, frees), (0, 0));
    }
    assert_ne!(surface.get_control("rate").unwrap(), json_number(1.0));
}

fn json_number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

#[test]
fn a_pending_jump_reads_clamped_through_nested_developments() {
    let leaf = json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }],
        "connections": [],
        "controls": [{ "key": "lvl", "module": "a", "control": "level" }]
    });
    let outer = json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": leaf }],
        "modules": [{ "id": "n", "type": "inner" }],
        "connections": [],
        "controls": [{ "key": "lvl", "module": "n", "control": "lvl" }]
    });
    let built = factory("outer", outer).build(48_000, &json!({})).unwrap();
    let lvl = built.control_surface.unwrap().automation("lvl").unwrap();
    lvl.write_number(2.0);
    assert_eq!(lvl.current(), Some(1.0), "as the leaf dial will hold it");
}

#[test]
fn a_choice_fans_out_by_option_name_whatever_each_alias_numbers_it() {
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "dial" }, { "id": "b", "type": "dial" }],
        "connections": [],
        "controls": [
            { "key": "form", "module": "a", "control": "shape" },
            { "key": "form", "module": "b", "control": "slope" },
            { "key": "b_slope", "module": "b", "control": "slope" }
        ]
    });
    let (_graph, surface) = running_from(
        factory("choices", definition),
        json!({ "form": "flat" }),
    );
    assert_eq!(surface.get_control("form").unwrap(), "flat".into());
    assert_eq!(surface.get_control("b_slope").unwrap(), "flat".into());
    surface.set_control("form", "steep".into()).unwrap();
    assert_eq!(surface.get_control("b_slope").unwrap(), "steep".into());
    // An option one alias lacks changes neither.
    assert!(surface.set_control("form", "gentle".into()).is_err());
    assert_eq!(surface.get_control("b_slope").unwrap(), "steep".into());
}
