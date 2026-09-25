//! Topology compilation for the [`SignalGraph`]: processing order, feedback
//! groups, compiled routes, and output buffers. Runs only when the topology
//! changes, never on the audio hot path.

use super::scc::tarjan_scc;
use super::{CompiledRoute, ProcessGroup, SignalGraph};
use crate::MAX_BLOCK;

impl SignalGraph {
    /// Recomputes topological order, SCC process groups, compiled routes,
    /// input connectivity, output buffers, and sink indices.
    ///
    /// Called only when topology changes, never on the audio hot path.
    pub(super) fn recompile(&mut self) {
        let n = self.modules.len();

        // Build adjacency using module indices.
        let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
        let mut has_self_loop = vec![false; n];
        for edge in &self.edges {
            let Some(from_idx) = self.modules.get_index_of(edge.from_module.as_str()) else {
                continue;
            };
            let Some(to_idx) = self.modules.get_index_of(edge.to_module.as_str()) else {
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
        for from_idx in 0..n {
            let targets = self
                .modules
                .get_index(from_idx)
                .map(|(_, inst)| inst.module().control_targets())
                .unwrap_or_default();
            for target in targets {
                let Some(to_idx) = self.modules.get_index_of(target.as_str()) else {
                    continue;
                };
                downstream[from_idx].push(to_idx);
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
                    match state[next] {
                        0 => {
                            state[next] = 1;
                            stack.push((next, 0));
                        }
                        _ => {
                            // Back-edge (on stack) or already finished — skip.
                        }
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
        self.process_order = order;

        // Strongly-connected components (Tarjan), in topological order.
        let (comp_id, comps) = tarjan_scc(&downstream, n);

        // Rebuild per-destination compiled route lists. Resolve port names to
        // indices once here; classify back-edges for one-sample feedback delay.
        self.compiled_routes = (0..n).map(|_| Vec::new()).collect();
        for edge in &self.edges {
            let Some(from_idx) = self.modules.get_index_of(edge.from_module.as_str()) else {
                continue;
            };
            let Some(to_idx) = self.modules.get_index_of(edge.to_module.as_str()) else {
                continue;
            };
            let Some((_, from_module)) = self.modules.get_index(from_idx) else {
                continue;
            };
            let Some((_, to_module)) = self.modules.get_index(to_idx) else {
                continue;
            };
            let Some(from_port) = from_module
                .module()
                .output_port_index(edge.from_port.as_str())
            else {
                continue;
            };
            let Some(to_port) = to_module.module().input_port_index(edge.to_port.as_str()) else {
                continue;
            };
            // A back-edge connects two modules in the same SCC where the source
            // is processed at or after the destination within the per-sample
            // order, so the destination reads the source's previous sample.
            let delayed = comp_id[from_idx] == comp_id[to_idx] && pos[from_idx] >= pos[to_idx];
            self.compiled_routes[to_idx].push(CompiledRoute {
                from_module: from_idx,
                from_port,
                to_port,
                delayed,
            });
        }

        // Reset input connectivity and clear input buffers, then mark connected
        // ports. Connectivity lets modules arbitrate signal-vs-control default.
        // Only the active block span is read, so zeroing the full MAX_BLOCK would
        // be wasted work on the audio thread (recompile runs here).
        let clear = self.block_size.clamp(1, MAX_BLOCK);
        for mi in 0..n {
            let n_in = self
                .modules
                .get_index(mi)
                .map(|(_, m)| m.module().inputs().len())
                .unwrap_or(0);
            if let Some((_, inst)) = self.modules.get_index_mut(mi) {
                let module = inst.module_mut();
                for p in 0..n_in {
                    module.input_block_mut(p)[..clear].fill(0.0);
                    module.set_input_connected(p, false);
                }
            }
        }
        self.connected_in_ports = (0..n).map(|_| Vec::new()).collect();
        for ti in 0..n {
            let route_count = self.compiled_routes[ti].len();
            for r in 0..route_count {
                let to_port = self.compiled_routes[ti][r].to_port;
                if let Some((_, inst)) = self.modules.get_index_mut(ti) {
                    inst.module_mut().set_input_connected(to_port, true);
                }
                if !self.connected_in_ports[ti].contains(&to_port) {
                    self.connected_in_ports[ti].push(to_port);
                }
            }
        }

        // Build process groups: SCCs in topological order, members ordered by
        // their position in the per-sample order.
        self.process_groups = comps
            .into_iter()
            .map(|mut members| {
                members.sort_by_key(|&m| pos[m]);
                let feedback =
                    members.len() > 1 || (members.len() == 1 && has_self_loop[members[0]]);
                ProcessGroup { members, feedback }
            })
            .collect();

        // Allocate per-module output block buffers and carry storage.
        self.out_counts = (0..n)
            .map(|mi| {
                self.modules
                    .get_index(mi)
                    .map(|(_, m)| m.module().outputs().len())
                    .unwrap_or(0)
            })
            .collect();
        let cap = self.block_size.clamp(1, MAX_BLOCK);
        self.block_capacity = cap;
        self.out_bufs = self
            .out_counts
            .iter()
            .map(|&c| vec![0.0; c * cap])
            .collect();
        self.out_prev = self.out_counts.iter().map(|&c| vec![0.0; c]).collect();

        // Cache sink indices.
        self.sink_indices = self
            .sinks
            .iter()
            .filter_map(|id| self.modules.get_index_of(id.as_str()))
            .collect();
    }
}
