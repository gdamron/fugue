//! A development exposing an inner payload control: the payload is handed
//! on whole to its one alias, and the key reads back as last written.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::test_support::legacy_dial::LegacyDialFactory;
use crate::test_support::tape::TapeFactory;

fn factory(controls: serde_json::Value) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(TapeFactory);
    registry.register(LegacyDialFactory);
    let definition = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "a", "type": "tape", "config": { "notes": [1] } },
            { "id": "b", "type": "tape" },
            { "id": "l", "type": "legacy_dial" }
        ],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "out" }],
        "controls": controls
    });
    DevelopmentFactory {
        name: "reel".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

/// The development exposing `a`'s notes, built with `config`, running
/// alone in an offline graph.
fn running(config: serde_json::Value) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    let controls = json!([{ "name": "notes", "module": "a", "control": "notes" }]);
    let built = factory(controls).build(48_000, &config).unwrap();
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
        "reel",
        built.module,
        Some(surface.clone()),
    )
    .unwrap();
    (graph, surface)
}

fn output(graph: &Arc<Mutex<SignalGraph>>) -> f32 {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["reel"].module_mut();
    module.process(1);
    module.output_block(0)[0]
}

#[test]
fn a_development_reads_its_payload_back_as_its_alias_was_built() {
    let (graph, surface) = running(json!({}));
    assert_eq!(surface.get_control("notes").unwrap(), "[1.0]".into());
    assert_eq!(surface.controls()[0].default, "[1.0]".into());
    assert_eq!(output(&graph), 1.0);
}

#[test]
fn a_development_hands_its_payload_to_its_one_alias() {
    let (graph, surface) = running(json!({ "notes": "[2, 3]" }));
    assert_eq!(output(&graph), 5.0, "from its config");
    surface.set_control("notes", "[4]".into()).unwrap();
    assert_eq!(output(&graph), 4.0, "at once, offline");
    assert_eq!(surface.get_control("notes").unwrap(), "[4.0]".into());
    assert_eq!(surface.controls()[0].default, "[4.0]".into(), "listed too");
    assert!(surface.set_control("notes", "nope".into()).is_err());
    assert_eq!(surface.get_control("notes").unwrap(), "[4.0]".into());
}

#[test]
fn a_payload_key_reaching_two_controls_is_refused() {
    let both = json!([
        { "name": "notes", "module": "a", "control": "notes" },
        { "name": "notes", "module": "b", "control": "notes" }
    ]);
    // A legacy alias (a legacy dial's waveform) may not share it either.
    let legacy = json!([
        { "name": "notes", "module": "l", "control": "waveform" },
        { "name": "notes", "module": "a", "control": "notes" }
    ]);
    for controls in [both, legacy] {
        let error = factory(controls).build(48_000, &json!({})).err().unwrap();
        assert!(error.to_string().contains("exactly one"), "{error}");
    }
}

#[test]
fn a_developments_config_writes_a_payload_before_the_scalars_beside_it() {
    // The scalar's key sorts before the payload's, which carries a gain.
    let controls = json!([
        { "name": "a_gain", "module": "a", "control": "gain" },
        { "name": "notes", "module": "a", "control": "notes" }
    ]);
    let config = json!({ "a_gain": 0.5, "notes": r#"{"notes": [2], "gain": 0.25}"# });
    let built = factory(controls).build(48_000, &config).unwrap();
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
        "reel",
        built.module,
        built.control_surface,
    )
    .unwrap();
    assert_eq!(
        output(&graph),
        1.0,
        "two notes at the gain written beside them"
    );
}

#[test]
fn a_development_has_an_asset_built_by_its_aliass_own_surface() {
    use crate::test_support::sampler::{wav, SamplerFactory};
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let mut registry = ModuleRegistry::default();
    registry.register(SamplerFactory);
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "s", "type": "sampler", "config": { "samples": [a, a] } }],
        "connections": [],
        "outputs": [{ "name": "frames", "from": "s", "from_port": "slot.1" }],
        "controls": [{ "name": "sample", "module": "s", "control": "asset.1" }]
    });
    let factory = DevelopmentFactory {
        name: "pad".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    };
    // Written from its config while building, then live offline: each time
    // loaded with the sampler's own rate, which the development lacks.
    let mut built = factory.build(48_000, &json!({ "sample": b })).unwrap();
    let surface = built.control_surface.take().unwrap();
    let module = built.module.module_mut();
    surface.bind(crate::invention::declared::Route::Inner, module);
    module.process(1);
    assert_eq!(module.output_block(0)[0], 960.0);
    assert_eq!(surface.get_control("sample").unwrap(), b.as_str().into());
    let error = surface
        .set_control("sample", "nope.wav".into())
        .unwrap_err();
    assert!(error.contains("nope.wav"), "{error}");
    assert_eq!(surface.get_control("sample").unwrap(), b.as_str().into());
}
