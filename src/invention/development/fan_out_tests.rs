//! A development key reaching the payload control of several modules
//! that load what it names: each alias loads its own, the audio thread
//! hands each on without allocating or freeing, and what each replaces is
//! retired with them.

use std::collections::HashSet;
use std::thread;

use serde_json::json;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::test_support::sampler::{wav, SamplerFactory, SAMPLER};

/// Two samplers, `p` and `q`, each holding `a` in both slots, with
/// `controls` exposed.
fn factory(a: &str, controls: serde_json::Value) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(SamplerFactory);
    let definition = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "p", "type": SAMPLER, "config": { "samples": [a, a] } },
            { "id": "q", "type": SAMPLER, "config": { "samples": [a, a] } }
        ],
        "connections": [],
        "outputs": [
            { "name": "p", "from": "p", "from_port": "slot.0" },
            { "name": "q", "from": "q", "from_port": "slot.0" }
        ],
        "controls": controls
    });
    DevelopmentFactory {
        name: "pair".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

fn both() -> serde_json::Value {
    json!([
        { "name": "sample", "module": "p", "control": "asset.0" },
        { "name": "sample", "module": "q", "control": "asset.0" }
    ])
}

/// Built with `config`, running alone in an offline graph.
fn running(
    factory: DevelopmentFactory,
    config: serde_json::Value,
) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    running_after(factory, config, |_| {})
}

/// [`running`], with `written` writing its surface before it runs.
fn running_after(
    factory: DevelopmentFactory,
    config: serde_json::Value,
    written: impl FnOnce(&ControlSurfaceInstance),
) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    let built = factory.build(48_000, &config).unwrap();
    let surface = built.control_surface.unwrap();
    written(&surface);
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

/// Each sampler's first slot's length, in engine-rate frames.
fn lengths(graph: &Arc<Mutex<SignalGraph>>) -> (f32, f32) {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    module.process(1);
    (module.output_block(0)[0], module.output_block(1)[0])
}

#[test]
fn one_sample_key_loads_into_every_player_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let c = wav(dir.path(), "c.wav", 1764);
    let (graph, surface) = running(factory(&a, both()), json!({ "sample": b }));
    assert_eq!(lengths(&graph), (960.0, 960.0), "from its config");

    surface.set_control("sample", c.as_str().into()).unwrap();
    assert_eq!(lengths(&graph), (1920.0, 1920.0));
    assert_eq!(surface.get_control("sample").unwrap(), c.as_str().into());

    let missing = dir.path().join("missing.wav");
    let error = surface.set_control("sample", missing.to_string_lossy().as_ref().into());
    assert!(error.unwrap_err().contains("missing.wav"));
    assert_eq!(lengths(&graph), (1920.0, 1920.0), "neither changed");
    assert_eq!(surface.get_control("sample").unwrap(), c.as_str().into());
}

#[test]
fn a_fanned_out_swap_neither_allocates_nor_frees_on_the_audio_thread() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let built = factory(&a, both()).build(48_000, &json!({})).unwrap();
    let surface = built.control_surface.unwrap();
    let mut module = built.module;
    let index = surface.declaration("sample").unwrap().index;
    let (payload, _) = surface
        .prepare_payload("sample", &b.as_str().into())
        .unwrap()
        .unwrap();

    let module = module.module_mut();
    let retired = thread::scope(|scope| {
        scope
            .spawn(|| {
                let (retired, allocs, frees) = allocator_events(|| {
                    let _audio = AudioThreadScope::enter();
                    let retired = module.apply_payload(index, payload).ok().unwrap();
                    module.process(1);
                    retired
                });
                assert_eq!((allocs, frees), (0, 0));
                retired
            })
            .join()
            .unwrap()
    });
    // Both replaced samples go with it, freed here.
    drop(retired);
    assert_eq!(
        (module.output_block(0)[0], module.output_block(1)[0]),
        (960.0, 960.0)
    );
}

/// `all` reaching both players, `q_sample` the second alone.
fn overlapping() -> serde_json::Value {
    json!([
        { "name": "all", "module": "p", "control": "asset.0" },
        { "name": "all", "module": "q", "control": "asset.0" },
        { "name": "q_sample", "module": "q", "control": "asset.0" }
    ])
}

#[test]
fn overlapping_keys_written_before_the_development_runs_each_land_in_write_order() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let c = wav(dir.path(), "c.wav", 1764);
    let config = json!({ "all": b, "q_sample": c });
    let (graph, _) = running(factory(&a, overlapping()), config);
    assert_eq!(lengths(&graph), (960.0, 1920.0), "from its config");

    let (graph, _) = running_after(factory(&a, overlapping()), json!({}), |surface| {
        surface.set_control("q_sample", c.as_str().into()).unwrap();
        surface.set_control("all", b.as_str().into()).unwrap();
    });
    assert_eq!(lengths(&graph), (960.0, 960.0), "the later write wins");
}

#[test]
fn a_refused_overlapping_write_keeps_what_was_written_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let missing = dir.path().join("missing.wav");
    let (graph, surface) = running_after(factory(&a, overlapping()), json!({}), |surface| {
        surface.set_control("all", b.as_str().into()).unwrap();
        let refused = surface.set_control("q_sample", missing.to_string_lossy().as_ref().into());
        assert!(refused.unwrap_err().contains("missing.wav"));
    });
    assert_eq!(lengths(&graph), (960.0, 960.0));
    assert_eq!(surface.get_control("all").unwrap(), b.as_str().into());
}
