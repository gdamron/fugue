//! Preparing changes and installing what they compile, with a hand-linked
//! graph standing in for the publisher.

use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, SyncSender};

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::graph::{AudioLink, InputWrite, Mailbox, MasterObservers, SignalGraph};
use crate::{Invention, InventionBuilder};

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

fn edge(from: &str, from_port: &str, to: &str, to_port: &str) -> RoutingConnection {
    RoutingConnection {
        from_module: from.to_string(),
        from_port: from_port.to_string(),
        to_module: to.to_string(),
        to_port: to_port.to_string(),
    }
}

/// A graph linked to bare channel ends, with the mirror and surface
/// directory a publisher would keep. The test thread plays both sides.
struct Harness {
    graph: SignalGraph,
    mirror: TopologyMirror,
    directory: SurfaceDirectory,
    registry: ModuleRegistry,
    publications: Arc<Mailbox<Publication>>,
    _inputs: SyncSender<InputWrite>,
    retired: Receiver<Box<Publication>>,
}

impl Harness {
    fn new() -> Self {
        let document = Invention::from_json(BASE).unwrap();
        let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
        // A linked graph never receives on its command channel.
        let (_, commands) = mpsc::channel();
        let mut graph = SignalGraph::new(
            runtime.modules,
            runtime.sinks,
            runtime.routing,
            commands,
            MasterObservers::default(),
        );
        graph.recompile();
        let publications = Arc::new(Mailbox::new());
        let (inputs, input_rx) = mpsc::sync_channel(4);
        let (retire, retired) = mpsc::sync_channel(4);
        let applied = Arc::new(AtomicU64::new(0));
        graph.link = Some(AudioLink::new(
            publications.clone(),
            input_rx,
            retire,
            applied,
        ));
        Self {
            mirror: TopologyMirror::of(&graph.modules, &graph.edges),
            graph,
            directory: runtime.control_surfaces,
            registry: runtime.registry,
            publications,
            _inputs: inputs,
            retired,
        }
    }

    fn build(&self, id: &str, module_type: &str, config: serde_json::Value) -> BuiltModule {
        GraphChange::build(&self.registry, SAMPLE_RATE, id, module_type, &config).unwrap()
    }

    fn begin(&self) -> GraphChange {
        GraphChange::new(
            0,
            self.mirror.clone(),
            self.directory.clone(),
            self.graph.block_size,
        )
    }

    /// Prepares `change` and hands its publication to the graph, keeping
    /// the mirror and directory as a publisher would.
    fn publish(&mut self, change: GraphChange) {
        while self.retired.try_recv().is_ok() {}
        let prepared = change.prepare().unwrap();
        {
            let mut directory = self.directory.lock().unwrap();
            for id in self.mirror.modules.keys() {
                if !prepared.mirror.modules.contains_key(id) {
                    directory.shift_remove(id);
                }
            }
            for (id, module) in &prepared.built {
                match &module.surface {
                    Some(surface) => {
                        directory.insert(id.clone(), surface.clone());
                    }
                    None => {
                        directory.shift_remove(id);
                    }
                }
            }
        }
        self.mirror = prepared.mirror;
        if let Some(publication) = prepared.publication {
            assert!(self.publications.put(publication).is_none());
        }
    }

    /// Counts the install of the pending publication and the two blocks
    /// after it, which must neither allocate nor free nor recompile.
    fn assert_clean_install(&mut self, label: &str) {
        let ((), allocs, frees) = allocator_events(|| self.graph.ensure_process_order());
        assert_eq!((allocs, frees), (0, 0), "{label}: install");
        assert!(!self.graph.topo_dirty, "{label}: fell back to recompiling");
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        for block in 0..2 {
            let ((), allocs, frees) =
                allocator_events(|| self.graph.process_block(&mut left, &mut right));
            assert_eq!((allocs, frees), (0, 0), "{label}: block {block}");
        }
    }

    fn ids(&self) -> Vec<&str> {
        self.graph.modules.keys().map(String::as_str).collect()
    }

    fn routes_into(&self, id: &str) -> usize {
        let idx = self.graph.modules.get_index_of(id).unwrap();
        self.graph.compiled_routes[idx].len()
    }
}

#[test]
fn a_mixed_change_installs_in_one_block_without_allocating() {
    let mut harness = Harness::new();
    harness.assert_clean_install("first blocks");
    let mut change = harness.begin();
    change.upsert(
        "osc3",
        harness.build(
            "osc3",
            "oscillator",
            serde_json::json!({ "frequency": 330.0 }),
        ),
    );
    change.remove("osc2");
    change.upsert(
        "osc1",
        harness.build(
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
    harness.publish(change);

    harness.assert_clean_install("mixed change");
    assert_eq!(harness.ids(), ["osc1", "dac", "osc3"]);
    assert_eq!(harness.routes_into("dac"), 1);
    assert_eq!(harness.routes_into("osc1"), 1);
}

#[test]
fn sinks_are_replaced_added_and_removed_cleanly() {
    let mut harness = Harness::new();
    let mut change = harness.begin();
    change.upsert("dac", harness.build("dac", "dac", serde_json::json!({})));
    harness.publish(change);
    harness.assert_clean_install("replace the dac");

    let mut change = harness.begin();
    change.upsert("dac2", harness.build("dac2", "dac", serde_json::json!({})));
    change
        .connect(edge("osc2", "audio", "dac2", "audio"))
        .unwrap();
    harness.publish(change);
    harness.assert_clean_install("add a second sink");
    assert_eq!(harness.graph.sink_indices.len(), 2);

    let mut change = harness.begin();
    change.remove("dac2");
    harness.publish(change);
    harness.assert_clean_install("remove a sink");
    assert_eq!(harness.graph.sink_indices.len(), 1);
}

#[test]
fn feedback_loops_are_made_and_broken_cleanly() {
    let mut harness = Harness::new();
    let cycle = [
        edge("osc1", "audio", "osc2", "fm"),
        edge("osc2", "audio", "osc1", "fm"),
        edge("osc1", "audio", "osc1", "am"),
    ];
    let mut change = harness.begin();
    for e in &cycle {
        change.connect(e.clone()).unwrap();
    }
    harness.publish(change);
    harness.assert_clean_install("make a 2-cycle and a self-loop");
    assert!(harness.graph.process_groups.iter().any(|g| g.feedback));

    let mut change = harness.begin();
    for e in &cycle {
        change.disconnect(e.clone());
    }
    harness.publish(change);
    harness.assert_clean_install("break both loops");
    assert!(harness.graph.process_groups.iter().all(|g| !g.feedback));
}

#[test]
fn a_scheduler_publishes_cleanly_with_a_target_added_alongside() {
    let mut harness = Harness::new();
    let mut change = harness.begin();
    change.upsert(
        "sched",
        harness.build(
            "sched",
            "control_scheduler",
            serde_json::json!({
                "schedule": [
                    { "at": 0, "module": "osc3", "control": "frequency", "value": 330.0 },
                    { "at": 1, "module": "osc3", "control": "frequency", "value": 660.0, "ramp": 4 }
                ]
            }),
        ),
    );
    change.upsert(
        "osc3",
        harness.build("osc3", "oscillator", serde_json::json!({})),
    );
    change.upsert(
        "clock",
        harness.build("clock", "clock", serde_json::json!({ "bpm": 120.0 })),
    );
    change
        .connect(edge("clock", "gate", "sched", "gate"))
        .unwrap();
    change
        .connect(edge("osc3", "audio", "dac", "audio"))
        .unwrap();
    harness.publish(change);
    harness.assert_clean_install("add a scheduler with its target");

    // The scheduler's target is an ordering dependency: it runs first.
    let at = |id: &str| {
        let idx = harness.graph.modules.get_index_of(id).unwrap();
        harness
            .graph
            .process_order
            .iter()
            .position(|&i| i == idx)
            .unwrap()
    };
    assert!(at("sched") < at("osc3"));

    // The scheduler survives while its target is replaced.
    let mut change = harness.begin();
    change.upsert(
        "osc3",
        harness.build(
            "osc3",
            "oscillator",
            serde_json::json!({ "waveform": "saw" }),
        ),
    );
    harness.publish(change);
    harness.assert_clean_install("replace a surviving scheduler's target");
}

#[test]
fn code_and_agent_modules_come_and_go_cleanly() {
    let mut harness = Harness::new();
    let mut change = harness.begin();
    change.upsert("code", harness.build("code", "code", serde_json::json!({})));
    change.upsert(
        "agent",
        harness.build("agent", "agent", serde_json::json!({})),
    );
    harness.publish(change);
    harness.assert_clean_install("add code and agent modules");

    let mut change = harness.begin();
    change.remove("code");
    change.remove("agent");
    harness.publish(change);
    harness.assert_clean_install("remove code and agent modules");
    assert_eq!(harness.ids(), ["osc1", "osc2", "dac"]);
}

#[test]
fn a_failed_edit_or_preparation_publishes_nothing() {
    let harness = Harness::new();
    let build = |module_type: &str, config| {
        GraphChange::build(&harness.registry, SAMPLE_RATE, "x", module_type, &config)
    };
    assert!(matches!(
        build("no_such_type", serde_json::json!({})),
        Err(GraphCommandError::UnknownModuleType(_))
    ));
    assert!(matches!(
        build("lfo", serde_json::json!({ "waveform": "bogus" })),
        Err(GraphCommandError::ModuleBuildFailed(_))
    ));

    let mut change = harness.begin();
    assert!(matches!(
        change.connect(edge("osc1", "nope", "dac", "audio")),
        Err(GraphCommandError::InvalidPort(_))
    ));
    assert!(matches!(
        change.connect(edge("ghost", "audio", "dac", "audio")),
        Err(GraphCommandError::UnknownModule(_))
    ));
    change.upsert(
        "sched",
        harness.build(
            "sched",
            "control_scheduler",
            serde_json::json!({
                "schedule": [{ "at": 0, "module": "missing", "control": "frequency", "value": 1.0 }]
            }),
        ),
    );
    assert!(matches!(
        change.prepare(),
        Err(GraphCommandError::ModuleBuildFailed(_))
    ));
    assert!(harness.publications.take().is_none());
    assert!(!harness.directory.lock().unwrap().contains_key("sched"));
}

#[test]
fn a_change_that_edits_nothing_compiles_nothing() {
    let harness = Harness::new();
    let mut change = harness.begin();
    change.remove("missing");
    change.disconnect(edge("osc1", "audio", "osc2", "fm"));
    let prepared = change.prepare().unwrap();
    assert!(prepared.is_empty());
    assert!(prepared.publication.is_none());
    assert_eq!(prepared.base_generation, 0);
}

#[test]
fn disconnecting_a_module_drops_every_edge_touching_it() {
    let mut harness = Harness::new();
    let mut change = harness.begin();
    change.connect(edge("osc2", "audio", "osc1", "fm")).unwrap();
    change.disconnect_module("osc1");
    harness.publish(change);
    harness.assert_clean_install("disconnect a module");
    assert_eq!(harness.routes_into("dac"), 1);
    assert_eq!(harness.routes_into("osc1"), 0);
}
