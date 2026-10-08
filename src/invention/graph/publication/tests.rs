//! The audio thread's side of a publication, driven with hand-built
//! publications: no publisher involved.

use std::sync::atomic::AtomicU64;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::graph::compile::compile_topology;
use crate::invention::graph::MasterObservers;
use crate::spsc::Ring;
use crate::ModuleRegistry;

mod never_waits;

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
        remap: SurvivorRemap::default(),
        generation: 0,
        absorbed: Vec::new(),
    })
}

/// The module order of [`base_graph`].
const BASE_IDS: [&str; 3] = ["osc1", "osc2", "dac"];

/// The control thread's ends of a graph's link.
struct ControlEnds {
    publications: Arc<Mailbox<Publication>>,
    inputs: Producer<InputWrite>,
    retired: Arc<Ring<Box<Publication>>>,
    applied: Arc<AtomicU64>,
}

fn link(graph: &mut SignalGraph, retire_capacity: usize) -> ControlEnds {
    let publications = Arc::new(Mailbox::new());
    let inputs = Ring::with_capacity(16);
    let retired = Ring::with_capacity(retire_capacity);
    let applied = Arc::new(AtomicU64::new(0));
    graph.link = Some(AudioLink::new(
        publications.clone(),
        Consumer::claim(Arc::clone(&inputs)),
        16,
        Producer::claim(Arc::clone(&retired)),
        applied.clone(),
    ));
    ControlEnds {
        publications,
        inputs: Producer::claim(inputs),
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

/// Replaces osc1, removes osc2, adds osc3, and rewires, mapped against the
/// base graph.
fn mixed_publication() -> Box<Publication> {
    let mut next = publication(
        vec![
            ("osc1", Next::Prepared(osc(220.0))),
            ("dac", Next::Survivor("dac")),
            ("osc3", Next::Prepared(osc(330.0))),
        ],
        vec![
            edge("osc3", "dac", "audio"),
            edge("osc3", "osc1", "frequency_mod"),
            edge("osc1", "dac", "audio"),
        ],
    );
    next.map_survivors(BASE_IDS);
    next
}

/// The feedback carry stored for module `id`.
fn carry(graph: &SignalGraph, id: &str) -> Vec<f32> {
    graph.out_prev[graph.modules.get_index_of(id).unwrap()].clone()
}

#[test]
fn installing_a_publication_neither_allocates_nor_frees() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    render(&mut graph, 2);
    let dac_carry = carry(&graph, "dac");
    assert!(dac_carry.iter().any(|v| *v != 0.0));
    drop(ends.publications.put(mixed_publication()));

    let ((), allocs, frees) = allocator_events(|| graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "installing touched the allocator");
    assert!(!graph.topo_dirty, "installing fell back to recompiling");
    assert_eq!(ids(&graph), ["osc1", "dac", "osc3"]);
    // The surviving dac's carry came across (at a new index); the rebuilt
    // osc1 and the new osc3 start from zero.
    assert_eq!(carry(&graph, "dac"), dac_carry);
    assert!(carry(&graph, "osc1").iter().all(|v| *v == 0.0));
    assert!(carry(&graph, "osc3").iter().all(|v| *v == 0.0));
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);

    // The next block runs the new topology without touching the allocator.
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(render(&mut graph, 1).iter().any(|sample| *sample != 0.0));

    // The old map, with the removed and replaced instances and the dac's
    // placeholder, comes back to be freed here.
    let retired = ends.retired.pop().unwrap();
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
    drop(ends.retired.pop().unwrap());
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
    let mut first = publication(
        vec![
            ("osc1", Next::Survivor("oscillator")),
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
            ("osc3", Next::Prepared(osc(330.0))),
        ],
        vec![edge("osc3", "dac", "audio")],
    );
    first.map_survivors(BASE_IDS);
    drop(ends.publications.put(first));
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
    next.map_survivors(["osc1", "osc2", "dac", "osc3"]);
    drop(next.absorb(ends.publications.take().unwrap(), false));
    assert_eq!(next.survivor_count(), 3);
    // The folded remap maps from the graph still running, which has no osc3.
    let remap: Vec<_> = next.remap.survivors().collect();
    assert_eq!(remap, [(0, 0), (1, 1), (2, 2)]);
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

/// The oscillators' `frequency` input index.
fn frequency_port(graph: &SignalGraph) -> usize {
    graph.modules["osc1"]
        .module()
        .input_port_index("frequency")
        .unwrap()
}

/// `osc1`'s whole `frequency` input block.
fn osc1_frequency(graph: &mut SignalGraph) -> Vec<f32> {
    let port = frequency_port(graph);
    let osc1 = graph.modules.get_mut("osc1").unwrap().module_mut();
    osc1.input_block_mut(port).to_vec()
}

#[test]
fn a_fallback_install_keeps_inputs_only_when_its_remap_vouches_for_them() {
    // A survivor missing from the running graph makes each install fall
    // back to recompiling, with or without a remap.
    let ghost = || {
        publication(
            vec![
                ("osc1", Next::Survivor("oscillator")),
                ("ghost", Next::Survivor("oscillator")),
                ("dac", Next::Survivor("dac")),
            ],
            vec![edge("osc1", "dac", "audio"), edge("ghost", "dac", "audio")],
        )
    };
    for mapped in [true, false] {
        let mut graph = base_graph();
        let mut ends = link(&mut graph, 4);
        let write = InputWrite {
            generation: 0,
            module_idx: 0,
            port_idx: frequency_port(&graph),
            value: 0.25,
        };
        ends.inputs.push(write).unwrap();
        render(&mut graph, 1);
        assert!(osc1_frequency(&mut graph).iter().all(|v| *v == 0.25));

        let mut next = ghost();
        if mapped {
            next.map_survivors(BASE_IDS);
        }
        drop(ends.publications.put(next));
        render(&mut graph, 1);
        assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
        // osc1's frequency stayed unconnected: it keeps the written value
        // through the install and the recompile, unless the remap cannot
        // vouch for osc1 being the instance it was written to.
        let expected = if mapped { 0.25 } else { 0.0 };
        assert!(
            osc1_frequency(&mut graph).iter().all(|v| *v == expected),
            "mapped: {mapped}"
        );
    }
}

#[test]
fn queued_input_writes_reach_the_module() {
    let mut graph = base_graph();
    let mut ends = link(&mut graph, 4);
    ends.inputs
        .push(InputWrite {
            generation: 0,
            module_idx: 0,
            port_idx: frequency_port(&graph),
            value: 0.25,
        })
        .unwrap();
    let ((), allocs, frees) = allocator_events(|| graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0));
    let port = frequency_port(&graph);
    let osc1 = graph.modules.get_mut("osc1").unwrap().module_mut();
    assert!(osc1.input_block_mut(port).iter().all(|v| *v == 0.25));
}

#[test]
fn folding_composes_survivor_remaps_against_the_running_graph() {
    // The first removes osc1 and rebuilds osc2; the second, prepared on top
    // of it, keeps both osc2 and dac as survivors and adds osc1 back.
    let mut first = publication(
        vec![
            ("osc2", Next::Prepared(osc(550.0))),
            ("dac", Next::Survivor("dac")),
        ],
        vec![edge("osc2", "dac", "audio")],
    );
    first.map_survivors(BASE_IDS);
    assert_eq!(first.remap.len(), 3);
    assert_eq!(
        (first.remap.get(0), first.remap.get(1), first.remap.get(2)),
        (None, None, Some(1))
    );

    let mut next = publication(
        vec![
            ("dac", Next::Survivor("dac")),
            ("osc2", Next::Survivor("oscillator")),
            ("osc1", Next::Prepared(osc(440.0))),
        ],
        vec![edge("osc2", "dac", "audio"), edge("osc1", "dac", "audio")],
    );
    next.map_survivors(["osc2", "dac"]);
    assert_eq!((next.remap.get(0), next.remap.get(1)), (Some(1), Some(0)));
    drop(next.absorb(first, false));

    // Only the running dac survives both: the running osc1 was removed, and
    // osc2 is the instance the first publication built.
    assert_eq!(next.remap.len(), 3);
    let remap: Vec<_> = next.remap.survivors().collect();
    assert_eq!(remap, [(2, 0)]);
    assert_eq!(next.survivor_count(), 1);
}

#[test]
fn an_unmapped_publication_carries_nothing() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    render(&mut graph, 2);
    let mut unmapped = mixed_publication();
    unmapped.remap = SurvivorRemap::default();
    drop(ends.publications.put(unmapped));

    // A remap that does not cover the running graph is not trusted: the
    // install stays clean and every carry starts from zero.
    let ((), allocs, frees) = allocator_events(|| graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0));
    assert!(!graph.topo_dirty);
    assert!(graph.out_prev.iter().flatten().all(|v| *v == 0.0));
}
