//! Topology compilation for the [`SignalGraph`]: processing order, feedback
//! groups, compiled routes, and output buffers.
//!
//! Compilation runs only when the topology changes, never on the audio hot
//! path. A live graph compiles on the control thread from the publisher's
//! mirror (see `crate::invention::publish`); offline render compiles in place
//! from the instances it owns.

use indexmap::IndexMap;

use super::chunks::{plan_groups, Edge};
use super::releases::Previous;
use super::scc::{is_back_edge, positions, process_groups, topological_order};
use super::{CompiledRoute, ProcessGroup, RoutingConnection, SignalGraph};
use crate::invention::runtime::ModuleInstance;
use crate::{FeedbackCut, MAX_BLOCK};

/// What compilation needs to know about a topology's modules, by index.
pub(crate) trait TopologyFacts {
    /// Number of modules.
    fn module_count(&self) -> usize;
    /// Index of the module with id `id`.
    fn index_of(&self, id: &str) -> Option<usize>;
    /// Index of `module`'s input port named `port`.
    fn input_index(&self, module: usize, port: &str) -> Option<usize>;
    /// Index of `module`'s output port named `port`.
    fn output_index(&self, module: usize, port: &str) -> Option<usize>;
    /// Number of output ports on `module`.
    fn output_count(&self, module: usize) -> usize;
    /// Ids of modules whose controls `module` writes while processing (see
    /// [`crate::Module::control_targets`]).
    fn control_targets(&self, module: usize) -> Vec<String>;
    /// The minimum delay `module` declares (see
    /// [`crate::Module::feedback_cut`]); `None` when it declares none, or
    /// when these facts only order modules and never run them.
    fn feedback_cut(&self, module: usize) -> Option<FeedbackCut> {
        let _ = module;
        None
    }
}

impl TopologyFacts for IndexMap<String, ModuleInstance> {
    fn module_count(&self) -> usize {
        self.len()
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.get_index_of(id)
    }

    fn input_index(&self, module: usize, port: &str) -> Option<usize> {
        self.get_index(module)?.1.module().input_port_index(port)
    }

    fn output_index(&self, module: usize, port: &str) -> Option<usize> {
        self.get_index(module)?.1.module().output_port_index(port)
    }

    fn output_count(&self, module: usize) -> usize {
        self.get_index(module)
            .map(|(_, m)| m.module().outputs().len())
            .unwrap_or(0)
    }

    fn control_targets(&self, module: usize) -> Vec<String> {
        self.get_index(module)
            .map(|(_, m)| m.module().control_targets())
            .unwrap_or_default()
    }

    fn feedback_cut(&self, module: usize) -> Option<FeedbackCut> {
        self.get_index(module)?.1.module().feedback_cut()
    }
}

/// The derived, index-based form of a topology that the hot path reads.
#[derive(Debug, Default)]
pub(crate) struct CompiledTopology {
    pub(crate) process_order: Vec<usize>,
    pub(crate) compiled_routes: Vec<Vec<CompiledRoute>>,
    pub(crate) connected_in_ports: Vec<Vec<usize>>,
    pub(crate) process_groups: Vec<ProcessGroup>,
    pub(crate) group_of: Vec<usize>,
    pub(crate) sink_indices: Vec<usize>,
    pub(crate) out_bufs: Vec<Vec<f32>>,
    pub(crate) out_prev: Vec<Vec<f32>>,
    pub(crate) out_counts: Vec<usize>,
    pub(crate) block_capacity: usize,
    /// Each module's ordering-only edges, by id, as compiled: the modules
    /// whose controls it writes and those an order puts after it (see
    /// [`TopologyFacts::control_targets`]).
    pub(crate) ordering: Vec<Vec<String>>,
    /// Whether feedback groups were planned in chunks (see
    /// [`super::chunks`]), which a recompile of the same topology keeps.
    pub(crate) chunked: bool,
}

impl CompiledTopology {
    /// Exchanges this topology with the graph's derived state. Swaps only:
    /// no allocation and no free, so it is safe on the audio thread.
    pub(crate) fn swap_with(&mut self, graph: &mut SignalGraph) {
        std::mem::swap(&mut self.process_order, &mut graph.process_order);
        std::mem::swap(&mut self.compiled_routes, &mut graph.compiled_routes);
        std::mem::swap(&mut self.connected_in_ports, &mut graph.connected_in_ports);
        std::mem::swap(&mut self.process_groups, &mut graph.process_groups);
        std::mem::swap(&mut self.group_of, &mut graph.group_of);
        std::mem::swap(&mut self.sink_indices, &mut graph.sink_indices);
        std::mem::swap(&mut self.out_bufs, &mut graph.out_bufs);
        std::mem::swap(&mut self.out_prev, &mut graph.out_prev);
        std::mem::swap(&mut self.out_counts, &mut graph.out_counts);
        std::mem::swap(&mut self.block_capacity, &mut graph.block_capacity);
        std::mem::swap(&mut self.ordering, &mut graph.ordering);
        std::mem::swap(&mut self.chunked, &mut graph.chunked);
        // Its modules or routes changed: their tables are found again.
        graph.tables_stale = true;
    }
}

/// Computes topological order, SCC process groups, compiled routes, input
/// connectivity, output buffers (sized for `block_size`), and sink indices.
///
/// Feedback groups run in chunks where they can when `chunked` (see
/// [`super::chunks`]). Edges whose endpoints or ports do not resolve are
/// skipped.
pub(crate) fn compile_topology(
    facts: &impl TopologyFacts,
    edges: &[RoutingConnection],
    sinks: &[String],
    block_size: usize,
    chunked: bool,
) -> CompiledTopology {
    let n = facts.module_count();

    // Build adjacency using module indices.
    let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    for edge in edges {
        let Some(from_idx) = facts.index_of(&edge.from_module) else {
            continue;
        };
        let Some(to_idx) = facts.index_of(&edge.to_module) else {
            continue;
        };
        downstream[from_idx].push(to_idx);
    }

    // Control-write targets are ordering dependencies without routes: a
    // module that writes another module's controls during `process()`
    // (see `Module::control_targets`) must run first so the write lands
    // in the same block. These edges join the adjacency for the DFS and
    // SCC passes but compile no routes; a resulting cycle simply becomes
    // a feedback group, processed sample-by-sample.
    let ordering: Vec<Vec<String>> = (0..n).map(|m| facts.control_targets(m)).collect();
    for (from_idx, edges_from) in downstream.iter_mut().enumerate() {
        for target in &ordering[from_idx] {
            let Some(to_idx) = facts.index_of(target) else {
                continue;
            };
            edges_from.push(to_idx);
        }
    }

    // Reverse post-order (topological order), used for intra-SCC member
    // ordering and back-edge classification, and the strongly-connected
    // components (Tarjan) as process groups, in topological order.
    let order = topological_order(&downstream);
    let pos = positions(&order);
    let (mut process_groups, group_of) = process_groups(&downstream, &pos);

    // Per-destination compiled route lists. Resolve port names to indices
    // once here; classify back-edges for one-sample feedback delay.
    let mut compiled_routes: Vec<Vec<CompiledRoute>> = (0..n).map(|_| Vec::new()).collect();
    for edge in edges {
        let Some(from_idx) = facts.index_of(&edge.from_module) else {
            continue;
        };
        let Some(to_idx) = facts.index_of(&edge.to_module) else {
            continue;
        };
        let Some(from_port) = facts.output_index(from_idx, &edge.from_port) else {
            continue;
        };
        let Some(to_port) = facts.input_index(to_idx, &edge.to_port) else {
            continue;
        };
        // A back-edge connects two modules in the same SCC where the source
        // is processed at or after the destination within the per-sample
        // order, so the destination reads the source's previous sample.
        let delayed = is_back_edge(&group_of, &pos, from_idx, to_idx);
        compiled_routes[to_idx].push(CompiledRoute {
            from_module: from_idx,
            from_port,
            to_port,
            delayed,
        });
    }

    // Feedback groups whose every cycle passes through a declared delay
    // run in chunks of it (see `chunks`).
    let route_edges = compiled_routes.iter().enumerate().flat_map(|(to, routes)| {
        routes.iter().map(move |route| Edge {
            from: route.from_module,
            to,
            port: Some(route.to_port),
        })
    });
    let ordering_edges = ordering.iter().enumerate().flat_map(|(from, targets)| {
        targets.iter().filter_map(move |target| {
            Some(Edge {
                from,
                to: facts.index_of(target)?,
                port: None,
            })
        })
    });
    plan_groups(
        chunked,
        &mut process_groups,
        &group_of,
        route_edges.chain(ordering_edges),
        |m| facts.feedback_cut(m),
    );

    // Distinct connected input ports per module. Connectivity lets modules
    // arbitrate signal-vs-control default.
    let connected_in_ports: Vec<Vec<usize>> = compiled_routes
        .iter()
        .map(|routes| {
            let mut ports: Vec<usize> = Vec::new();
            for route in routes {
                if !ports.contains(&route.to_port) {
                    ports.push(route.to_port);
                }
            }
            ports
        })
        .collect();

    // Per-module output block buffers and carry storage.
    let out_counts: Vec<usize> = (0..n).map(|mi| facts.output_count(mi)).collect();
    let block_capacity = block_size.clamp(1, MAX_BLOCK);
    let out_bufs = out_counts
        .iter()
        .map(|&c| vec![0.0; c * block_capacity])
        .collect();
    let out_prev = out_counts.iter().map(|&c| vec![0.0; c]).collect();

    let sink_indices = sinks.iter().filter_map(|id| facts.index_of(id)).collect();

    CompiledTopology {
        process_order: order,
        compiled_routes,
        connected_in_ports,
        process_groups,
        group_of,
        sink_indices,
        out_bufs,
        out_prev,
        out_counts,
        block_capacity,
        ordering,
        chunked,
    }
}

/// A graph's own modules with the ordering edges its topology was
/// published with, which a recompile in place keeps (see
/// [`SignalGraph::recompile`]).
struct Published<'a> {
    modules: &'a IndexMap<String, ModuleInstance>,
    ordering: &'a [Vec<String>],
}

impl TopologyFacts for Published<'_> {
    fn module_count(&self) -> usize {
        self.modules.module_count()
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.modules.index_of(id)
    }

    fn input_index(&self, module: usize, port: &str) -> Option<usize> {
        self.modules.input_index(module, port)
    }

    fn output_index(&self, module: usize, port: &str) -> Option<usize> {
        self.modules.output_index(module, port)
    }

    fn output_count(&self, module: usize) -> usize {
        self.modules.output_count(module)
    }

    fn control_targets(&self, module: usize) -> Vec<String> {
        self.ordering.get(module).cloned().unwrap_or_default()
    }

    fn feedback_cut(&self, module: usize) -> Option<FeedbackCut> {
        self.modules.feedback_cut(module)
    }
}

impl SignalGraph {
    /// Recompiles the derived topology from the graph's own modules, edges,
    /// and sinks, then brings input connectivity in line with it.
    ///
    /// While the previous connectivity still describes the modules by index
    /// (nothing was added or removed since: see [`Self::apply_command`],
    /// which forgets it otherwise), only ports whose connectivity changed are
    /// reset, so an unconnected input keeps the value last written to it
    /// across a block size change or an install that fell back to
    /// recompiling. Otherwise every input is reset.
    ///
    /// A topology a publication installed keeps the ordering edges it was
    /// compiled with from the publisher's mirror, the orders conditions
    /// wait on among them, rather than read each module's control targets
    /// again, so it stays the topology the mirror describes.
    ///
    /// Allocates: called before a live graph moves to the audio thread, by
    /// offline render, and as the audio thread's fallback when a block size
    /// change outdates the compiled buffers.
    pub(crate) fn recompile(&mut self) {
        let (modules, edges, sinks) = (&self.modules, &self.edges, &self.sinks);
        let published = Published {
            modules,
            ordering: &self.ordering,
        };
        let chunked = self.chunked;
        let mut topology = if self.published && self.ordering.len() == modules.len() {
            compile_topology(&published, edges, sinks, self.block_size, chunked)
        } else {
            compile_topology(modules, edges, sinks, self.block_size, chunked)
        };
        topology.swap_with(self);
        if let Some(watch) = &mut self.end_watch {
            watch.refresh(&self.modules);
        }
        // `topology` now holds the previous derived state.
        let same_order = topology.connected_in_ports.len() == self.modules.len();
        if !same_order {
            self.releases.clear();
        }
        for mi in 0..self.modules.len() {
            let was = topology.connected_in_ports.get(mi).filter(|_| same_order);
            let was = was.map(|ports| Previous {
                ports,
                routes: topology.compiled_routes.get(mi).map_or(&[], Vec::as_slice),
                carry: &topology.out_prev,
            });
            self.reset_module_inputs(mi, was);
        }
        self.topo_dirty = false;
    }

    /// Recompiles in place when module `module_id` now writes other
    /// modules' controls than the topology was compiled with, as an offline
    /// `schedule` write can leave a scheduler: a live one publishes the
    /// topology recompiled. The modules and routes stay, so each
    /// keeps its feedback carry, as a live recompile's survivors do. A
    /// topology already due to recompile is left to. Allocates: offline.
    pub(crate) fn recompile_if_retargeted(&mut self, module_id: &str) {
        let Some((index, _, instance)) = self.modules.get_full(module_id) else {
            return;
        };
        let targets = instance.module().control_targets();
        if self.topo_dirty || self.ordering.get(index) == Some(&targets) {
            return;
        }
        let carry = std::mem::take(&mut self.out_prev);
        self.recompile();
        for (now, was) in self.out_prev.iter_mut().zip(&carry) {
            if now.len() == was.len() {
                now.copy_from_slice(was);
            }
        }
    }

    /// Brings module `mi`'s inputs in line with the graph's connectivity.
    /// `was` is its connections before, for the same instance; `None`
    /// treats the module as starting fresh.
    ///
    /// A port connected now is declared connected; routing fills it each
    /// block. A port `was` shows connected that is unconnected now glides to
    /// rest (see [`super::releases`]) where it can, and is otherwise cleared
    /// and declared unconnected at once, as every port unconnected now is on
    /// a module starting fresh. A port unconnected before and now is left as
    /// it is, keeping any value written to it (see
    /// [`crate::Module::set_input`]) or the release it is in.
    ///
    /// Clearing covers the whole buffer, not just the active span, so an
    /// unconnected input never holds routed samples beyond it that a larger
    /// block size would later expose. Only ports that change are cleared, so
    /// this costs nothing for the modules an edit leaves alone.
    /// Allocation-free: it runs on the audio thread at each install.
    pub(super) fn reset_module_inputs(&mut self, mi: usize, was: Option<Previous>) {
        let ports = self
            .modules
            .get_index(mi)
            .map_or(0, |(_, inst)| inst.module().inputs().len());
        for p in 0..ports {
            let connected = self
                .connected_in_ports
                .get(mi)
                .is_some_and(|now| now.contains(&p));
            let changed = was.is_none_or(|was| was.ports.contains(&p));
            if !connected && !changed {
                continue;
            }
            if !self.releases.is_empty() {
                self.releases.cancel(mi, p);
            }
            if !connected && was.is_some_and(|was| self.release_input(mi, p, was)) {
                continue;
            }
            let Some((_, inst)) = self.modules.get_index_mut(mi) else {
                return;
            };
            let module = inst.module_mut();
            if connected {
                module.set_input_connected(p, true);
            } else {
                module.input_block_mut(p).fill(0.0);
                module.set_input_connected(p, false);
            }
        }
    }
}
