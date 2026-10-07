//! Topology compilation for the [`SignalGraph`]: processing order, feedback
//! groups, compiled routes, and output buffers.
//!
//! Compilation runs only when the topology changes, never on the audio hot
//! path. A live graph compiles on the control thread from the publisher's
//! mirror (see `crate::invention::publish`); offline render compiles in place
//! from the instances it owns.

use indexmap::IndexMap;

use super::scc::tarjan_scc;
use super::{CompiledRoute, ProcessGroup, RoutingConnection, SignalGraph};
use crate::invention::runtime::ModuleInstance;
use crate::MAX_BLOCK;

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
}

/// The derived, index-based form of a topology that the hot path reads.
#[derive(Debug, Default)]
pub(crate) struct CompiledTopology {
    pub(crate) process_order: Vec<usize>,
    pub(crate) compiled_routes: Vec<Vec<CompiledRoute>>,
    pub(crate) connected_in_ports: Vec<Vec<usize>>,
    pub(crate) process_groups: Vec<ProcessGroup>,
    pub(crate) sink_indices: Vec<usize>,
    pub(crate) out_bufs: Vec<Vec<f32>>,
    pub(crate) out_prev: Vec<Vec<f32>>,
    pub(crate) out_counts: Vec<usize>,
    pub(crate) block_capacity: usize,
}

impl CompiledTopology {
    /// Exchanges this topology with the graph's derived state. Swaps only:
    /// no allocation and no free, so it is safe on the audio thread.
    pub(crate) fn swap_with(&mut self, graph: &mut SignalGraph) {
        std::mem::swap(&mut self.process_order, &mut graph.process_order);
        std::mem::swap(&mut self.compiled_routes, &mut graph.compiled_routes);
        std::mem::swap(&mut self.connected_in_ports, &mut graph.connected_in_ports);
        std::mem::swap(&mut self.process_groups, &mut graph.process_groups);
        std::mem::swap(&mut self.sink_indices, &mut graph.sink_indices);
        std::mem::swap(&mut self.out_bufs, &mut graph.out_bufs);
        std::mem::swap(&mut self.out_prev, &mut graph.out_prev);
        std::mem::swap(&mut self.out_counts, &mut graph.out_counts);
        std::mem::swap(&mut self.block_capacity, &mut graph.block_capacity);
    }
}

/// Computes topological order, SCC process groups, compiled routes, input
/// connectivity, output buffers (sized for `block_size`), and sink indices.
///
/// Edges whose endpoints or ports do not resolve are skipped.
pub(crate) fn compile_topology(
    facts: &impl TopologyFacts,
    edges: &[RoutingConnection],
    sinks: &[String],
    block_size: usize,
) -> CompiledTopology {
    let n = facts.module_count();

    // Build adjacency using module indices.
    let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    let mut has_self_loop = vec![false; n];
    for edge in edges {
        let Some(from_idx) = facts.index_of(&edge.from_module) else {
            continue;
        };
        let Some(to_idx) = facts.index_of(&edge.to_module) else {
            continue;
        };
        downstream[from_idx].push(to_idx);
        if from_idx == to_idx {
            has_self_loop[from_idx] = true;
        }
    }

    // Control-write targets are ordering dependencies without routes: a
    // module that writes another module's controls during `process()`
    // (see `Module::control_targets`) must run first so the write lands
    // in the same block. These edges join the adjacency for the DFS and
    // SCC passes but compile no routes; a resulting cycle simply becomes
    // a feedback group, processed sample-by-sample.
    for (from_idx, edges_from) in downstream.iter_mut().enumerate() {
        for target in facts.control_targets(from_idx) {
            let Some(to_idx) = facts.index_of(&target) else {
                continue;
            };
            edges_from.push(to_idx);
            if from_idx == to_idx {
                has_self_loop[from_idx] = true;
            }
        }
    }

    // Iterative DFS producing reverse post-order (topological order). State:
    // 0 = unvisited, 1 = on stack (in progress), 2 = finished. Used for
    // intra-SCC member ordering and back-edge classification.
    let mut state = vec![0_u8; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    for start in 0..n {
        if state[start] != 0 {
            continue;
        }

        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;

        while let Some((node, idx)) = stack.last_mut() {
            let node = *node;
            if *idx < downstream[node].len() {
                let next = downstream[node][*idx];
                *idx += 1;
                // Back-edge (on stack) or already finished: skip.
                if state[next] == 0 {
                    state[next] = 1;
                    stack.push((next, 0));
                }
            } else {
                let (finished, _) = stack.pop().unwrap();
                state[finished] = 2;
                order.push(finished);
            }
        }
    }

    order.reverse();
    let mut pos = vec![0usize; n];
    for (i, &m) in order.iter().enumerate() {
        pos[m] = i;
    }

    // Strongly-connected components (Tarjan), in topological order.
    let (comp_id, comps) = tarjan_scc(&downstream, n);

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
        let delayed = comp_id[from_idx] == comp_id[to_idx] && pos[from_idx] >= pos[to_idx];
        compiled_routes[to_idx].push(CompiledRoute {
            from_module: from_idx,
            from_port,
            to_port,
            delayed,
        });
    }

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

    // Process groups: SCCs in topological order, members ordered by their
    // position in the per-sample order.
    let process_groups = comps
        .into_iter()
        .map(|mut members| {
            members.sort_by_key(|&m| pos[m]);
            let feedback = members.len() > 1 || (members.len() == 1 && has_self_loop[members[0]]);
            ProcessGroup { members, feedback }
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
        sink_indices,
        out_bufs,
        out_prev,
        out_counts,
        block_capacity,
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
    /// Allocates: called before a live graph moves to the audio thread, by
    /// offline render, and as the audio thread's fallback when a block size
    /// change outdates the compiled buffers.
    pub(crate) fn recompile(&mut self) {
        let mut topology =
            compile_topology(&self.modules, &self.edges, &self.sinks, self.block_size);
        topology.swap_with(self);
        // `topology` now holds the previous derived state.
        let previous = &topology.connected_in_ports;
        let same_order = previous.len() == self.modules.len();
        for mi in 0..self.modules.len() {
            let was = previous.get(mi).filter(|_| same_order);
            self.reset_module_inputs(mi, was.map(Vec::as_slice));
        }
        self.topo_dirty = false;
    }

    /// Brings module `mi`'s inputs in line with the graph's connectivity.
    /// `was` is the ports connected before, for the same instance; `None`
    /// treats the module as starting fresh.
    ///
    /// A port connected now is declared connected; routing fills it each
    /// block. A port unconnected now is cleared and declared unconnected,
    /// unless `was` shows it unconnected before too: that port is left as it
    /// is, keeping any value written to it (see [`crate::Module::set_input`]).
    ///
    /// Clearing covers the whole buffer, not just the active span, so an
    /// unconnected input never holds routed samples beyond it that a larger
    /// block size would later expose. Only ports that change are cleared, so
    /// this costs nothing for the modules an edit leaves alone.
    /// Allocation-free: it runs on the audio thread at each install.
    pub(super) fn reset_module_inputs(&mut self, mi: usize, was: Option<&[usize]>) {
        let Some((_, inst)) = self.modules.get_index_mut(mi) else {
            return;
        };
        let now = self
            .connected_in_ports
            .get(mi)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let module = inst.module_mut();
        for p in 0..module.inputs().len() {
            if now.contains(&p) {
                module.set_input_connected(p, true);
            } else if was.is_none_or(|was| was.contains(&p)) {
                module.input_block_mut(p).fill(0.0);
                module.set_input_connected(p, false);
            }
        }
    }
}
