use std::time::Duration;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::graph::MasterObservers;
use crate::invention::runtime::module_ports;
use crate::{Invention, InventionBuilder};

mod probe;

use probe::{DropProbeFactory, DROP_PROBE};

const SAMPLE_RATE: u32 = 48_000;

const BASE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "waveform": "sine", "frequency": 550.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// A live-linked graph driven by the test thread, standing in for the audio
/// thread.
struct Rig {
    graph: SignalGraph,
    live: LiveGraph,
    registry: ModuleRegistry,
}

impl Rig {
    fn new(json: &str) -> Self {
        let document = Invention::from_json(json).unwrap();
        let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
        let ports = Arc::new(Mutex::new(module_ports(&runtime.modules)));
        // Until the live editing paths publish, a graph still takes a
        // command receiver; a linked graph never receives on it.
        let (_, commands) = std::sync::mpsc::channel();
        let mut graph = SignalGraph::new(
            runtime.modules,
            runtime.sinks,
            runtime.routing,
            commands,
            MasterObservers::default(),
        );
        graph.recompile();
        let live = LiveGraph::link(&mut graph, runtime.state, runtime.control_surfaces, ports);
        Self {
            graph,
            live,
            registry: runtime.registry,
        }
    }

    fn build(&self, id: &str, module_type: &str, config: serde_json::Value) -> change::BuiltModule {
        GraphChange::build(&self.registry, SAMPLE_RATE, id, module_type, &config).unwrap()
    }

    fn render(&mut self, blocks: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        for _ in 0..blocks {
            self.graph.process_block(&mut left, &mut right);
            out.extend_from_slice(&left);
        }
        out
    }

    fn module_ids(&self) -> Vec<String> {
        self.graph.modules.keys().cloned().collect()
    }

    fn generation_and_applied(&self) -> (u64, u64) {
        let publisher = self.live.publisher().lock().unwrap();
        (publisher.generation(), publisher.applied())
    }

    /// A change that adds, removes, replaces, and rewires modules at once.
    fn mixed_change(&self) -> PreparedChange {
        let mut change = self.live.begin();
        change.upsert(
            "osc3",
            self.build(
                "osc3",
                "oscillator",
                serde_json::json!({ "frequency": 330.0 }),
            ),
        );
        change.remove("osc2");
        change.upsert(
            "osc1",
            self.build(
                "osc1",
                "oscillator",
                serde_json::json!({ "waveform": "square" }),
            ),
        );
        change.disconnect(edge("osc1", "audio", "dac", "audio"));
        change
            .connect(edge("osc3", "audio", "dac", "audio"))
            .unwrap();
        change.connect(edge("osc3", "audio", "osc1", "fm")).unwrap();
        change.prepare().unwrap()
    }
}

/// Everything a refused or failed change must leave as it was.
#[allow(clippy::type_complexity)]
fn snapshot(
    rig: &Rig,
) -> (
    IndexMap<String, RuntimeModuleInfo>,
    Vec<RuntimeConnectionInfo>,
    Option<crate::Invention>,
    Vec<String>,
    Vec<String>,
    (u64, u64),
) {
    let state = rig.live.state.lock().unwrap();
    (
        state.modules.clone(),
        state.connections.clone(),
        state.document.clone(),
        rig.live
            .control_surfaces
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect(),
        rig.live
            .module_ports
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect(),
        rig.generation_and_applied(),
    )
}

#[test]
fn a_multi_edit_change_reaches_the_audio_thread_as_one_publication() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let before = rig.generation_and_applied();
    let prepared = rig.mixed_change();
    rig.live.commit(prepared).unwrap();
    assert_eq!(rig.generation_and_applied(), (before.0 + 1, before.1));

    rig.render(1);
    assert_eq!(rig.generation_and_applied(), (before.0 + 1, before.1 + 1));
    // The whole change landed in that one block.
    assert_eq!(rig.module_ids(), ["osc1", "dac", "osc3"]);
    let routes_into = |id: &str| {
        let idx = rig.graph.modules.get_index_of(id).unwrap();
        rig.graph.compiled_routes[idx].len()
    };
    assert_eq!(routes_into("dac"), 1);
    assert_eq!(routes_into("osc1"), 1);
}

#[test]
fn untouched_modules_keep_their_phase_across_a_publication() {
    let mut edited = Rig::new(BASE);
    let mut untouched = Rig::new(BASE);
    assert_eq!(edited.render(5), untouched.render(5));

    // Add an unconnected module: every audible module survives the swap.
    let lfo = edited.build("lfo", "oscillator", serde_json::json!({ "frequency": 2.0 }));
    edited
        .live
        .edit(|change| {
            change.upsert("lfo", lfo);
            Ok(())
        })
        .unwrap();

    // Same output sample for sample as a rig that never published: no
    // survivor was reset by the install.
    assert_eq!(edited.render(20), untouched.render(20));
    assert_eq!(edited.generation_and_applied(), (1, 1));
}

#[test]
fn a_change_prepared_before_another_publication_is_refused() {
    let mut rig = Rig::new(BASE);
    let mut stale = rig.live.begin();
    stale.upsert(
        "osc3",
        rig.build("osc3", "oscillator", serde_json::json!({})),
    );
    stale.remove("osc2");
    stale
        .connect(edge("osc3", "audio", "dac", "audio"))
        .unwrap();
    let stale = stale.prepare().unwrap();

    // Another writer publishes first.
    rig.live
        .add_module(
            &rig.registry,
            SAMPLE_RATE,
            "osc4",
            "oscillator",
            &serde_json::json!({}),
        )
        .unwrap();
    rig.render(1);
    let before = snapshot(&rig);

    let refused = rig.live.commit(stale);
    assert!(matches!(refused, Err(GraphCommandError::TopologyMoved)));
    rig.render(1);
    assert_eq!(snapshot(&rig), before);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc4"]);
    let mirror: Vec<String> = {
        let publisher = rig.live.publisher().lock().unwrap();
        publisher.mirror().modules.keys().cloned().collect()
    };
    assert_eq!(mirror, ["osc1", "osc2", "dac", "osc4"]);
}

#[test]
fn concurrent_writers_all_land() {
    let mut rig = Rig::new(BASE);
    let writers: Vec<_> = (0..4)
        .map(|writer| {
            let live = rig.live.clone();
            let registry = rig.registry.clone();
            std::thread::spawn(move || {
                for n in 0..5 {
                    let id = format!("w{writer}_{n}");
                    live.add_module(
                        &registry,
                        SAMPLE_RATE,
                        &id,
                        "oscillator",
                        &serde_json::json!({}),
                    )
                    .unwrap();
                    live.connect(edge(&id, "audio", "dac", "audio")).unwrap();
                }
            })
        })
        .collect();
    while writers.iter().any(|writer| !writer.is_finished()) {
        rig.render(1);
    }
    for writer in writers {
        writer.join().unwrap();
    }
    rig.render(1);

    assert_eq!(rig.graph.modules.len(), 23);
    let dac = rig.graph.modules.get_index_of("dac").unwrap();
    assert_eq!(rig.graph.compiled_routes[dac].len(), 22);
    let publisher = rig.live.publisher().lock().unwrap();
    assert_eq!(publisher.mirror().modules.len(), 23);
    assert_eq!(rig.live.state.lock().unwrap().connections.len(), 22);
    assert_eq!(rig.live.module_ports.lock().unwrap().len(), 23);
}

#[test]
fn failed_preparation_leaves_everything_unchanged() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let before = snapshot(&rig);

    let unknown = rig.live.add_module(
        &rig.registry,
        SAMPLE_RATE,
        "x",
        "no_such_type",
        &serde_json::json!({}),
    );
    assert!(matches!(
        unknown,
        Err(GraphCommandError::UnknownModuleType(_))
    ));
    let bad_config = rig.live.add_module(
        &rig.registry,
        SAMPLE_RATE,
        "x",
        "lfo",
        &serde_json::json!({ "waveform": "bogus" }),
    );
    assert!(matches!(
        bad_config,
        Err(GraphCommandError::ModuleBuildFailed(_))
    ));
    let unresolved = rig.live.add_module(
        &rig.registry,
        SAMPLE_RATE,
        "sched",
        "control_scheduler",
        &serde_json::json!({
            "schedule": [{ "at": 0, "module": "missing", "control": "frequency", "value": 1.0 }]
        }),
    );
    assert!(matches!(
        unresolved,
        Err(GraphCommandError::ModuleBuildFailed(_))
    ));
    let bad_port = rig.live.connect(edge("osc1", "nope", "dac", "audio"));
    assert!(matches!(bad_port, Err(GraphCommandError::InvalidPort(_))));

    rig.render(1);
    assert_eq!(snapshot(&rig), before);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
}

#[test]
fn a_change_that_edits_nothing_publishes_nothing() {
    let mut rig = Rig::new(BASE);
    rig.live.remove_module("missing").unwrap();
    rig.live
        .disconnect(edge("osc1", "audio", "osc2", "fm"))
        .unwrap();
    let prepared = rig.live.begin().prepare().unwrap();
    assert!(prepared.is_empty());
    rig.live.commit(prepared).unwrap();
    rig.render(1);
    assert_eq!(rig.generation_and_applied(), (0, 0));
}

#[test]
fn a_swap_keeps_compatible_connections_only_when_asked() {
    let mut rig = Rig::new(BASE);
    let routes_into_dac = |rig: &Rig| {
        let dac = rig.graph.modules.get_index_of("dac").unwrap();
        rig.graph.compiled_routes[dac].len()
    };
    let swap = |rig: &Rig, preserve| {
        rig.live
            .swap_module(
                &rig.registry,
                SAMPLE_RATE,
                "osc1",
                "oscillator",
                &serde_json::json!({ "waveform": "square" }),
                preserve,
            )
            .unwrap()
    };
    let committed = swap(&rig, true);
    assert_eq!(committed.stopped, ["osc1"]);
    rig.render(1);
    assert_eq!(routes_into_dac(&rig), 2);

    swap(&rig, false);
    rig.render(1);
    assert_eq!(routes_into_dac(&rig), 1);
    assert!(matches!(
        rig.live.swap_module(
            &rig.registry,
            SAMPLE_RATE,
            "missing",
            "oscillator",
            &serde_json::json!({}),
            true,
        ),
        Err(GraphCommandError::UnknownModule(_))
    ));
}

#[test]
fn a_started_reclaimer_frees_removed_modules_off_the_audio_thread() {
    let probes = DropProbeFactory::default();
    let mut rig = Rig::new(BASE);
    rig.registry.register(probes.clone());
    let probe = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("probe", probe);
            change.connect(edge("osc1", "audio", "probe", "audio"))
        })
        .unwrap();
    rig.render(1);

    // The test thread stands in for the audio thread: installing the
    // removal frees nothing there, and no further change runs to free it.
    rig.live.remove_module("probe").unwrap();
    let ((), _, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!(frees, 0);
    assert!(probes.wait_for_drops(1, Duration::ZERO).is_empty());

    assert!(rig.live.start_reclaimer());
    let drops = probes.wait_for_drops(1, Duration::from_secs(2));
    assert_eq!(drops.len(), 1, "the removed probe was not freed");
    assert_ne!(drops[0], std::thread::current().id());
}

#[test]
fn a_full_input_queue_refuses_writes_until_the_audio_thread_drains_it() {
    let mut rig = Rig::new(BASE);
    let write = || InputWrite {
        module_id: "osc1".to_string(),
        port: "fm".to_string(),
        value: 0.0,
    };
    for _ in 0..publisher::INPUT_QUEUE_CAPACITY {
        rig.live.write_input(write()).unwrap();
    }
    assert!(matches!(
        rig.live.write_input(write()),
        Err(GraphCommandError::QueueFull)
    ));
    rig.render(1);
    rig.live.write_input(write()).unwrap();
}
