//! A development key reaching the same slot of two sample kits: each kit
//! loads what it names, so a hit on either plays the sample written.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::modules::SampleKitFactory;
use crate::test_support::sampler::wav;

/// Two kits, `p` and `q`, each with slot 36 holding `a`, sharing a gate
/// and a key, with `kick` reaching both kits' `asset.0`.
fn factory(a: &str) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(SampleKitFactory);
    let slot = json!({ "samples": [{ "key": 36, "asset": a }] });
    let definition = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "p", "type": "sample_kit", "config": slot },
            { "id": "q", "type": "sample_kit", "config": slot }
        ],
        "connections": [],
        "inputs": [
            { "name": "gate", "to": "p", "to_port": "play" },
            { "name": "gate", "to": "q", "to_port": "play" },
            { "name": "key", "to": "p", "to_port": "key" },
            { "name": "key", "to": "q", "to_port": "key" }
        ],
        "outputs": [
            { "name": "p", "from": "p", "from_port": "audio_left" },
            { "name": "q", "from": "q", "from_port": "audio_left" }
        ],
        "controls": [
            { "name": "kick", "module": "p", "control": "asset.0" },
            { "name": "kick", "module": "q", "control": "asset.0" }
        ]
    });
    DevelopmentFactory {
        name: "kits".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

/// Built with `config`, running alone in an offline graph.
fn running(
    a: &str,
    config: serde_json::Value,
) -> (Arc<Mutex<SignalGraph>>, ControlSurfaceInstance) {
    let built = factory(a).build(48_000, &config).unwrap();
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
        "kits",
        built.module,
        Some(surface.clone()),
    )
    .unwrap();
    (graph, surface)
}

/// Hits slot 36 on both kits and returns how many engine-rate frames each
/// then sounds for: the length of the sample its slot holds.
fn hit(graph: &Arc<Mutex<SignalGraph>>) -> (usize, usize) {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["kits"].module_mut();
    module.set_input("key", 36.0).unwrap();
    module.set_input("gate", 0.0).unwrap();
    module.process(1);
    module.set_input("gate", 1.0).unwrap();
    let mut sounding = (0, 0);
    for _ in 0..40 {
        module.process(64);
        let count = |output: usize| {
            let block = &module.output_block(output)[..64];
            block.iter().filter(|level| **level > 0.25).count()
        };
        sounding = (sounding.0 + count(0), sounding.1 + count(1));
    }
    sounding
}

/// Within a few frames of `frames`, as resampling softens a sample's edges.
fn about(frames: usize) -> std::ops::RangeInclusive<usize> {
    frames - 4..=frames + 4
}

#[test]
fn one_sample_key_loads_into_every_kit_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let c = wav(dir.path(), "c.wav", 1764);
    let (graph, surface) = running(&a, json!({ "kick": b }));
    let (p, q) = hit(&graph);
    assert!(
        about(960).contains(&p) && about(960).contains(&q),
        "from its config: {p}, {q}"
    );

    surface.set_control("kick", c.as_str().into()).unwrap();
    let (p, q) = hit(&graph);
    assert!(
        about(1920).contains(&p) && about(1920).contains(&q),
        "{p}, {q}"
    );
    assert_eq!(surface.get_control("kick").unwrap(), c.as_str().into());

    let missing = dir.path().join("missing.wav");
    let error = surface.set_control("kick", missing.to_string_lossy().as_ref().into());
    assert!(error.unwrap_err().contains("missing.wav"));
    let (p, q) = hit(&graph);
    assert!(
        about(1920).contains(&p) && about(1920).contains(&q),
        "neither changed: {p}, {q}"
    );
    assert_eq!(surface.get_control("kick").unwrap(), c.as_str().into());
}
