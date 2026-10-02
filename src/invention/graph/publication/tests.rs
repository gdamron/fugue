//! The audio thread's side of a publication, driven with hand-built
//! publications: no publisher involved.

use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, SyncSender};

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::graph::compile::compile_topology;
use crate::invention::graph::MasterObservers;
use crate::ModuleRegistry;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 64;

fn build(module_type: &str, config: serde_json::Value) -> ModuleInstance {
    ModuleRegistry::default()
        .build(module_type, SAMPLE_RATE, &config)
        .unwrap()
        .module
}

fn osc(frequency: f32) -> ModuleInstance {
    build(
        "oscillator",
        serde_json::json!({ "waveform": "sine", "frequency": frequency }),
    )
}

fn edge(from: &str, to: &str, to_port: &str) -> RoutingConnection {
    RoutingConnection {
        from_module: from.to_string(),
        from_port: "audio".to_string(),
        to_module: to.to_string(),
        to_port: to_port.to_string(),
    }
}

fn sinks_of(modules: &IndexMap<String, ModuleInstance>) -> Vec<String> {
    modules
        .iter()
        .filter(|(_, m)| matches!(m, GraphModule::Sink(_)))
        .map(|(id, _)| id.clone())
        .collect()
}

/// `osc1` and `osc2` into a dac, compiled as a graph about to go live.
fn base_graph() -> SignalGraph {
    let mut modules = IndexMap::new();
    modules.insert("osc1".to_string(), osc(440.0));
    modules.insert("osc2".to_string(), osc(550.0));
    modules.insert("dac".to_string(), build("dac", serde_json::json!({})));
    let sinks = sinks_of(&modules);
    let edges = vec![edge("osc1", "dac", "audio"), edge("osc2", "dac", "audio")];
    let mut graph = SignalGraph::new(modules, sinks, edges, MasterObservers::default());
    graph.recompile();
    graph
}

/// A module in a hand-built publication: a survivor placeholder, or a
/// prepared instance.
enum Next {
    Survivor(&'static str),
    Prepared(ModuleInstance),
}

/// Builds a publication of `next` (in final order) wired by `edges`. Facts
/// for survivors come from fresh twins of the given type; the twins are
/// dropped here, on the test thread, exactly as a publisher would.
fn publication(next: Vec<(&str, Next)>, edges: Vec<RoutingConnection>) -> Box<Publication> {
    let mut facts = IndexMap::new();
    let mut survivor = Vec::new();
    for (id, module) in next {
        let (instance, is_survivor) = match module {
            Next::Survivor(module_type) => (build(module_type, serde_json::json!({})), true),
            Next::Prepared(instance) => (instance, false),
        };
        facts.insert(id.to_string(), instance);
        survivor.push(is_survivor);
    }
    let sinks = sinks_of(&facts);
    let topology = compile_topology(&facts, &edges, &sinks, crate::DEFAULT_BLOCK_SIZE);
    let modules = facts
        .into_iter()
        .zip(&survivor)
        .map(|((id, instance), &is_survivor)| (id, if is_survivor { vacant() } else { instance }))
        .collect();
    Box::new(Publication {
        modules,
        survivor,
        sinks,
        edges,
        topology,
    })
}

/// The control thread's ends of a graph's link.
struct ControlEnds {
    publications: Arc<Mailbox<Publication>>,
    inputs: SyncSender<InputWrite>,
    retired: Receiver<Box<Publication>>,
    applied: Arc<AtomicU64>,
}

fn link(graph: &mut SignalGraph, retire_capacity: usize) -> ControlEnds {
    let publications = Arc::new(Mailbox::new());
    let (inputs, input_rx) = mpsc::sync_channel(16);
    let (retire, retired) = mpsc::sync_channel(retire_capacity);
    let applied = Arc::new(AtomicU64::new(0));
    graph.link = Some(AudioLink::new(
        publications.clone(),
        input_rx,
        retire,
        applied.clone(),
    ));
    ControlEnds {
        publications,
        inputs,
        retired,
        applied,
    }
}

fn render(graph: &mut SignalGraph, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut left = [0.0f32; FRAMES];
    let mut right = [0.0f32; FRAMES];
    for _ in 0..blocks {
        graph.process_block(&mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out
}

/// One block on the "audio thread", counting its allocator events.
fn counted_block(graph: &mut SignalGraph) -> (usize, usize) {
    let mut left = [0.0f32; FRAMES];
    let mut right = [0.0f32; FRAMES];
    let ((), allocs, frees) = allocator_events(|| graph.process_block(&mut left, &mut right));
    (allocs, frees)
}

fn ids(graph: &SignalGraph) -> Vec<&str> {
    graph.modules.keys().map(String::as_str).collect()
}

/// Replaces osc1, removes osc2, adds osc3, and rewires.
fn mixed_publication() -> Box<Publication> {
    publication(
        vec![
            ("osc1", Next::Prepared(osc(220.0))),
            ("dac", Next::Survivor("dac")),
            ("osc3", Next::Prepared(osc(330.0))),
        ],
        vec![
            edge("osc3", "dac", "audio"),
            edge("osc3", "osc1", "fm"),
            edge("osc1", "dac", "audio"),
        ],
    )
}

#[test]
fn installing_a_publication_neither_allocates_nor_frees() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    render(&mut graph, 2);
    drop(ends.publications.put(mixed_publication()));

    let ((), allocs, frees) = allocator_events(|| graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "installing touched the allocator");
    assert!(!graph.topo_dirty, "installing fell back to recompiling");
    assert_eq!(ids(&graph), ["osc1", "dac", "osc3"]);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);

    // The next block runs the new topology without touching the allocator.
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(render(&mut graph, 1).iter().any(|sample| *sample != 0.0));

    // The old map, with the removed and replaced instances and the dac's
    // placeholder, comes back to be freed here.
    let retired = ends.retired.try_recv().unwrap();
    let mut old: Vec<&str> = retired.modules.keys().map(String::as_str).collect();
    old.sort_unstable();
    assert_eq!(old, ["dac", "osc1", "osc2"]);
    assert_eq!(retired.modules["dac"].module().name(), "vacant");
}

#[test]
fn survivors_keep_their_state_across_a_publication() {
    let mut edited = base_graph();
    let mut untouched = base_graph();
    let ends = link(&mut edited, 4);
    assert_eq!(render(&mut edited, 5), render(&mut untouched, 5));

    // Every module survives and the edges are unchanged.
    drop(ends.publications.put(publication(
        vec![
            ("osc1", Next::Survivor("oscillator")),
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
        ],
        vec![edge("osc1", "dac", "audio"), edge("osc2", "dac", "audio")],
    )));

    // Same output sample for sample as a graph that never published.
    assert_eq!(render(&mut edited, 20), render(&mut untouched, 20));
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
}

#[test]
fn a_full_retire_channel_holds_one_retirement_and_takes_nothing_more() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 1);
    let survivors = || {
        publication(
            vec![
                ("osc1", Next::Survivor("oscillator")),
                ("osc2", Next::Survivor("oscillator")),
                ("dac", Next::Survivor("dac")),
            ],
            vec![edge("osc1", "dac", "audio"), edge("osc2", "dac", "audio")],
        )
    };

    // The first retirement fills the channel; the second is held.
    drop(ends.publications.put(survivors()));
    assert_eq!(counted_block(&mut graph), (0, 0));
    drop(ends.publications.put(survivors()));
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert_eq!(ends.applied.load(Ordering::Relaxed), 2);

    // While one is held, nothing more is taken, and blocks stay clean.
    drop(ends.publications.put(mixed_publication()));
    for _ in 0..3 {
        assert_eq!(counted_block(&mut graph), (0, 0));
    }
    assert_eq!(ends.applied.load(Ordering::Relaxed), 2);
    assert_eq!(ids(&graph), ["osc1", "osc2", "dac"]);

    // Draining makes room: the held retirement goes, then the waiting
    // publication installs, all in one clean block.
    drop(ends.retired.try_recv().unwrap());
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert_eq!(ends.applied.load(Ordering::Relaxed), 3);
    assert_eq!(ids(&graph), ["osc1", "dac", "osc3"]);
}

#[test]
fn an_untaken_publication_folds_into_the_next() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);

    // The first adds osc3; the second, prepared on top of it, keeps osc3 as
    // a survivor and adds osc4.
    drop(ends.publications.put(publication(
        vec![
            ("osc1", Next::Survivor("oscillator")),
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
            ("osc3", Next::Prepared(osc(330.0))),
        ],
        vec![edge("osc3", "dac", "audio")],
    )));
    let mut next = publication(
        vec![
            ("osc1", Next::Survivor("oscillator")),
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
            ("osc3", Next::Survivor("oscillator")),
            ("osc4", Next::Prepared(osc(660.0))),
        ],
        vec![edge("osc3", "dac", "audio"), edge("osc4", "dac", "audio")],
    );
    drop(next.absorb(ends.publications.take().unwrap()));
    assert_eq!(next.survivor_count(), 3);
    drop(ends.publications.put(next));

    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(!graph.topo_dirty);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
    assert_eq!(ids(&graph), ["osc1", "osc2", "dac", "osc3", "osc4"]);
    assert_eq!(graph.modules["osc3"].module().name(), "Oscillator");
}

#[test]
fn a_survivor_missing_from_the_running_graph_falls_back_to_recompiling() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    drop(ends.publications.put(publication(
        vec![
            ("osc1", Next::Survivor("oscillator")),
            ("ghost", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
        ],
        vec![edge("ghost", "dac", "audio")],
    )));
    render(&mut graph, 1);
    // The fallback compiled from the instances, so the placeholder's
    // missing ports route nothing instead of being indexed.
    assert!(!graph.topo_dirty);
    let ghost = graph.modules.get_index_of("ghost").unwrap();
    assert!(graph
        .compiled_routes
        .iter()
        .flatten()
        .all(|r| r.from_module != ghost));
}

#[test]
fn queued_input_writes_reach_the_module() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    ends.inputs
        .try_send(InputWrite {
            module_id: "osc1".to_string(),
            port: "frequency".to_string(),
            value: 0.25,
        })
        .unwrap();
    let ((), allocs, _) = allocator_events(|| graph.ensure_process_order());
    assert_eq!(allocs, 0);
    let osc1 = graph.modules.get_mut("osc1").unwrap().module_mut();
    let port = osc1.input_port_index("frequency").unwrap();
    assert!(osc1.input_block_mut(port).iter().all(|v| *v == 0.25));
}
