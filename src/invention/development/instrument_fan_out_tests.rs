//! A development key reaching the same zone of two sample instruments:
//! each instrument loads what it names, so a note on either plays the
//! sample written, and a note saved in the development's config plays
//! once as it runs.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};
use crate::modules::SampleInstrumentFactory;
use crate::test_support::sampler::wav;

/// Two instruments, `p` and `q`, each with one zone over every key holding
/// `a`, with `sample` reaching both zones' `asset.0` and `note` both
/// instruments' `note_on`.
fn factory(a: &str) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(SampleInstrumentFactory);
    let zone = json!({ "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": a }] });
    let definition = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "p", "type": "sample_instrument", "config": zone },
            { "id": "q", "type": "sample_instrument", "config": zone }
        ],
        "connections": [],
        "outputs": [
            { "name": "p", "from": "p", "from_port": "audio_left" },
            { "name": "q", "from": "q", "from_port": "audio_left" }
        ],
        "controls": [
            { "name": "sample", "module": "p", "control": "asset.0" },
            { "name": "sample", "module": "q", "control": "asset.0" },
            { "name": "note", "module": "p", "control": "note_on" },
            { "name": "note", "module": "q", "control": "note_on" }
        ]
    });
    DevelopmentFactory {
        name: "pair".to_string(),
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
        "pair",
        built.module,
        Some(surface.clone()),
    )
    .unwrap();
    (graph, surface)
}

/// The samples' level (`wav` writes half scale), which one note plays at.
const ONE_NOTE: f32 = 0.5;

/// How many engine-rate frames each instrument sounds for from now: the
/// length of the sample its zone holds, once a note has started it. No
/// frame is louder than one note.
fn sounding(graph: &Arc<Mutex<SignalGraph>>) -> (usize, usize) {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    let mut sounding = (0, 0);
    for _ in 0..40 {
        module.process(64);
        let count = |output: usize| {
            let block = &module.output_block(output)[..64];
            let loudest = block
                .iter()
                .fold(0.0f32, |most, level| most.max(level.abs()));
            assert!(loudest < ONE_NOTE + 0.01, "one note at a time: {loudest}");
            block
                .iter()
                .filter(|level| **level > ONE_NOTE / 2.0)
                .count()
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
fn one_sample_key_loads_into_every_instrument_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 882));
    let c = wav(dir.path(), "c.wav", 1764);
    let (graph, surface) = running(&a, json!({ "sample": b, "note": 60 }));
    let (p, q) = sounding(&graph);
    assert!(
        about(960).contains(&p) && about(960).contains(&q),
        "its saved note plays its configured sample: {p}, {q}"
    );
    assert_eq!(sounding(&graph), (0, 0), "the saved note played once");

    surface.set_control("sample", c.as_str().into()).unwrap();
    surface.set_control("note", 60.0.into()).unwrap();
    let (p, q) = sounding(&graph);
    assert!(
        about(1920).contains(&p) && about(1920).contains(&q),
        "{p}, {q}"
    );
    assert_eq!(surface.get_control("sample").unwrap(), c.as_str().into());

    let missing = dir.path().join("missing.wav");
    let error = surface.set_control("sample", missing.to_string_lossy().as_ref().into());
    assert!(error.unwrap_err().contains("missing.wav"));
    surface.set_control("note", 60.0.into()).unwrap();
    let (p, q) = sounding(&graph);
    assert!(
        about(1920).contains(&p) && about(1920).contains(&q),
        "neither changed: {p}, {q}"
    );
}

#[test]
fn every_note_written_through_the_development_before_it_runs_plays() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "a.wav", 4410);
    let built = factory(&a).build(48_000, &json!({})).unwrap();
    let surface = built.control_surface.clone().unwrap();
    // A chord, written while the development is still building.
    for note in [60.0, 64.0, 67.0] {
        surface.set_control("note", note.into()).unwrap();
    }
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let surfaces = Mutex::new(IndexMap::new());
    add_offline(&graph, &surfaces, "pair", built.module, Some(surface)).unwrap();
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    module.process(64);
    let heard = module.output_block(0)[63];
    assert!(
        (heard - 3.0 * ONE_NOTE).abs() < 0.01,
        "the whole chord: {heard}"
    );
}
