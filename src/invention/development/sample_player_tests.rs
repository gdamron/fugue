//! Two sample players behind one development: one `sample` key loads into
//! both, and an exposed `play` restored from a saved config starts each
//! once, even where a player's own config also starts it.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::invention::declared::add_offline;
use crate::invention::graph::{MasterObservers, SignalGraph};

/// A 48 kHz mono file of `frames` frames, each at `level`.
fn tone(dir: &std::path::Path, name: &str, level: f32, frames: usize) -> String {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..frames {
        writer.write_sample(level).unwrap();
    }
    writer.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// Players `p` and `q` holding `a`, `q` starting on its own, with `sample`
/// and `start` reaching both.
fn pair(a: &str) -> DevelopmentFactory {
    let definition = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "p", "type": "sample_player", "config": { "asset": a, "loop": true } },
            {
                "id": "q",
                "type": "sample_player",
                "config": { "asset": a, "loop": true, "autoplay": true }
            }
        ],
        "connections": [],
        "outputs": [
            { "name": "p", "from": "p", "from_port": "audio_left" },
            { "name": "q", "from": "q", "from_port": "audio_left" },
            { "name": "p_start", "from": "p", "from_port": "start" },
            { "name": "q_start", "from": "q", "from_port": "start" }
        ],
        "controls": [
            { "name": "sample", "module": "p", "control": "asset" },
            { "name": "sample", "module": "q", "control": "asset" },
            { "name": "start", "module": "p", "control": "play" },
            { "name": "start", "module": "q", "control": "play" }
        ]
    });
    DevelopmentFactory {
        name: "pair".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry: ModuleRegistry::default(),
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

/// Built with `config`, running alone in an offline graph.
fn running(
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

/// One frame: each player's level, and whether each started on it.
fn frame(graph: &Arc<Mutex<SignalGraph>>) -> [f32; 4] {
    let mut graph = graph.lock().unwrap();
    let module = graph.modules["pair"].module_mut();
    module.process(1);
    [0, 1, 2, 3].map(|port| module.output_block(port)[0])
}

#[test]
fn a_saved_start_plays_both_players_once_and_one_sample_key_loads_into_both() {
    let dir = tempfile::tempdir().unwrap();
    let a = tone(dir.path(), "a.wav", 0.2, 64);
    let b = tone(dir.path(), "b.wav", 0.4, 64);
    let c = tone(dir.path(), "c.wav", 0.6, 64);

    let (graph, surface) = running(pair(&a), json!({ "sample": b, "start": true }));
    assert_eq!(
        frame(&graph),
        [0.4, 0.4, 1.0, 1.0],
        "each starts once, on b"
    );
    for _ in 0..8 {
        assert_eq!(frame(&graph), [0.4, 0.4, 0.0, 0.0], "and not again");
    }
    assert_eq!(surface.get_control("sample").unwrap(), b.as_str().into());

    surface.set_control("sample", c.as_str().into()).unwrap();
    assert_eq!(frame(&graph), [0.6, 0.6, 1.0, 1.0], "c swapped into both");

    // Without a saved start, only the player starting on its own plays.
    let (graph, _) = running(pair(&a), json!({}));
    assert_eq!(frame(&graph), [0.0, 0.2, 0.0, 1.0]);
}
