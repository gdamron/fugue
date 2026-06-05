//! Signal processing graph with pre-computed topological processing order.
//!
//! # Architecture Overview
//!
//! ## Signal Routing
//!
//! The system uses **named ports** for connections:
//! - Each module declares its inputs/outputs via the `Module` trait
//! - Connections specify port names: `{"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"}`
//! - All signals are f32 values - modules interpret them based on which port receives them
//!
//! ## Processing Order
//!
//! The system uses a **topological sort** computed when the graph topology changes:
//!
//! 1. **Topo sort** - DFS-based sort determines dependency order (runs only on topology change)
//! 2. **Linear iteration** - Each sample iterates the sorted order, setting inputs from upstream outputs
//! 3. **No recursion** - All modules processed in a single pass with zero per-sample allocations
//! 4. **Cycle handling** - Back-edges are skipped during DFS; those edges read one-sample-delayed values
//! 5. **Mix outputs** - Combine all sink outputs and return the final sample
//!
//! ## Routing Compilation
//!
//! At topology change we compile the string-keyed edge list into index-based
//! `CompiledRoute`s. The hot path then traverses `Vec<Vec<CompiledRoute>>` and
//! `IndexMap::get_index`/`get_index_mut` (both O(1) vector access) — no
//! `HashMap` string hashing per sample.
//!
//! ## Why IndexMap?
//!
//! **CRITICAL**: We use `IndexMap` instead of `HashMap` for deterministic iteration order.
//!
//! - HashMap has non-deterministic iteration order in Rust (depends on internal hash state)
//! - This caused race conditions where ADSR envelopes would work ~50% of the time
//! - IndexMap preserves insertion order (order from JSON definition), ensuring consistent behavior
//! - While the dependency graph handles ordering for connected modules, IndexMap ensures
//!   tie-breaking (when multiple valid orders exist) is deterministic across runs

use indexmap::IndexMap;
use std::sync::mpsc;

use crate::{GraphModule, MAX_BLOCK};

use super::runtime::ModuleInstance;

/// A command that can be sent to the audio thread for graph mutation.
pub(crate) enum GraphCommand {
    /// Set a module's input port to a specific value.
    SetModuleInput {
        module_id: String,
        port: String,
        value: f32,
    },
    /// Add a new module to the graph (overwrites if duplicate ID).
    AddModule {
        module_id: String,
        module: ModuleInstance,
    },
    /// Remove a module from the graph (fire-and-forget).
    RemoveModule { module_id: String },
    /// Add a connection between two modules.
    AddConnection {
        from_module: String,
        from_port: String,
        to_module: String,
        to_port: String,
    },
    /// Remove a connection between two modules (fire-and-forget).
    RemoveConnection {
        from_module: String,
        from_port: String,
        to_module: String,
        to_port: String,
    },
}

/// A single routing connection in the signal graph, by module name.
///
/// This is the authoritative string-keyed form used for topology-change
/// operations. The hot path uses [`CompiledRoute`] instead.
#[derive(Debug, Clone)]
pub(crate) struct RoutingConnection {
    pub(crate) from_module: String,
    pub(crate) from_port: String,
    pub(crate) to_module: String,
    pub(crate) to_port: String,
}

/// A pre-compiled route used on the audio hot path.
///
/// `from_module` indexes into the graph's `IndexMap<String, ModuleInstance>`
/// (via `get_index`/`get_index_mut`). Port indices are resolved at topology
/// change via `Module::input_port_index` / `output_port_index`, so the hot
/// path does no string hashing or matching.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CompiledRoute {
    pub(crate) from_module: usize,
    pub(crate) from_port: usize,
    pub(crate) to_port: usize,
    /// True if this edge is a feedback (back) edge within a strongly-connected
    /// component: the destination reads the source's *previous* sample
    /// (one-sample delay), exactly mirroring the legacy per-sample behavior.
    pub(crate) delayed: bool,
}

/// A group of modules processed together, in topological order of the
/// strongly-connected-component condensation.
///
/// A trivial, acyclic group (`feedback == false`, single member) is processed a
/// whole block at a time — the fast path. A feedback group (a real cycle, or a
/// self-loop) is processed sample-by-sample so that back-edges observe a
/// one-sample delay, preserving feedback fidelity regardless of block size.
#[derive(Debug, Clone)]
pub(crate) struct ProcessGroup {
    /// Member module indices, ordered by their position in `process_order`.
    pub(crate) members: Vec<usize>,
    /// Whether this group contains a feedback cycle (or self-loop).
    pub(crate) feedback: bool,
}

/// The signal processing graph for modular routing.
pub(crate) struct SignalGraph {
    /// All modules in the graph (including sinks).
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Sink modules that drive processing and collect output (by module id).
    pub(crate) sinks: Vec<String>,
    /// Authoritative edge list — updated by topology-change commands.
    pub(crate) edges: Vec<RoutingConnection>,
    /// Current sample number.
    pub(crate) current_sample: u64,
    /// Receiver for commands from the main thread.
    pub(crate) command_rx: mpsc::Receiver<GraphCommand>,
    /// Pre-computed topological processing order as module indices. Used for
    /// intra-SCC member ordering and back-edge classification.
    pub(crate) process_order: Vec<usize>,
    /// Pre-compiled routes indexed by destination module index.
    /// `compiled_routes[i]` is the list of edges feeding the i-th module.
    pub(crate) compiled_routes: Vec<Vec<CompiledRoute>>,
    /// Distinct connected input port indices per module. The hot path zeros
    /// these before summing routes, so multiple sources into one port mix
    /// (e.g. several voices into a DAC `audio` port).
    pub(crate) connected_in_ports: Vec<Vec<usize>>,
    /// Process groups (SCC condensation) in topological order.
    pub(crate) process_groups: Vec<ProcessGroup>,
    /// Sink module indices, cached for the hot path.
    pub(crate) sink_indices: Vec<usize>,
    /// Per-module output block buffers, port-major with stride `block_capacity`:
    /// `out_bufs[module][port * block_capacity + frame]`. The single shared
    /// surface every route reads from, so full-block and sample-by-sample
    /// processing interoperate.
    pub(crate) out_bufs: Vec<Vec<f32>>,
    /// Final sample of the previous block for each module output port, used by
    /// one-sample-delayed feedback reads at frame 0 of the next block.
    pub(crate) out_prev: Vec<Vec<f32>>,
    /// Output port count per module (parallel to module index).
    pub(crate) out_counts: Vec<usize>,
    /// Allocated per-port frame capacity of `out_bufs` (equals `block_size`).
    pub(crate) block_capacity: usize,
    /// Configurable processing block size in frames (always `<= MAX_BLOCK`).
    pub(crate) block_size: usize,
    /// Flag indicating topology changed and derived state needs recomputation.
    pub(crate) topo_dirty: bool,
}

impl SignalGraph {
    pub(crate) fn ensure_process_order(&mut self) {
        self.drain_commands();
        if self.topo_dirty {
            self.recompile();
            self.topo_dirty = false;
        }
    }

    /// Sets the processing block size (clamped to `[1, MAX_BLOCK]`) and marks
    /// derived buffers for reallocation. Not the audio hot path.
    pub(crate) fn set_block_size(&mut self, block_size: usize) {
        let block_size = block_size.clamp(1, MAX_BLOCK);
        if block_size != self.block_size {
            self.block_size = block_size;
            self.topo_dirty = true;
        }
    }

    /// Processes a block of `frames` frames (`frames == left.len() == right.len()`,
    /// always `<= block_size`), mixing all sink outputs into `left`/`right`.
    ///
    /// Process groups run in topological order: acyclic modules a whole block at
    /// a time, feedback cycles sample-by-sample. Zero heap allocations.
    pub(crate) fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.ensure_process_order();

        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }
        let frames = frames.min(self.block_capacity);
        self.current_sample += frames as u64;

        for g in 0..self.process_groups.len() {
            if self.process_groups[g].feedback {
                self.process_feedback_group(g, frames);
            } else {
                let module_idx = self.process_groups[g].members[0];
                self.process_full_block(module_idx, frames);
            }
        }

        // Mix sink output blocks.
        for i in 0..frames {
            left[i] = 0.0;
            right[i] = 0.0;
        }
        let sink_count = self.sink_indices.len();
        for si in 0..sink_count {
            let sink_idx = self.sink_indices[si];
            if let Some((_, inst)) = self.modules.get_index(sink_idx) {
                if let Some((l, r)) = inst.sink_block() {
                    for i in 0..frames {
                        left[i] += l[i];
                        right[i] += r[i];
                    }
                }
            }
        }
        if sink_count > 1 {
            let gain = 1.0 / (sink_count as f32).sqrt();
            for i in 0..frames {
                left[i] *= gain;
                right[i] *= gain;
            }
        }

        self.store_carry(frames);
    }

    /// Processes a single acyclic module a whole block at a time.
    fn process_full_block(&mut self, module_idx: usize, frames: usize) {
        // Zero connected input ports, then sum every route feeding them.
        let port_count = self.connected_in_ports[module_idx].len();
        for pi in 0..port_count {
            let port = self.connected_in_ports[module_idx][pi];
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                inst.module_mut().input_block_mut(port)[..frames].fill(0.0);
            }
        }

        let route_count = self.compiled_routes[module_idx].len();
        for r in 0..route_count {
            let route = self.compiled_routes[module_idx][r];
            let base = route.from_port * self.block_capacity;
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                let dst = inst.module_mut().input_block_mut(route.to_port);
                let src = &self.out_bufs[route.from_module][base..base + frames];
                for k in 0..frames {
                    dst[k] += src[k];
                }
            }
        }

        if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
            inst.module_mut().process(frames);
        }

        let n_out = self.out_counts[module_idx];
        for p in 0..n_out {
            let base = p * self.block_capacity;
            if let Some((_, inst)) = self.modules.get_index(module_idx) {
                let src = inst.module().output_block(p);
                self.out_bufs[module_idx][base..base + frames].copy_from_slice(&src[..frames]);
            }
        }
    }

    /// Processes a feedback group sample-by-sample so back-edges observe a
    /// one-sample delay, preserving feedback fidelity regardless of block size.
    fn process_feedback_group(&mut self, group: usize, frames: usize) {
        for s in 0..frames {
            let member_count = self.process_groups[group].members.len();
            for mi in 0..member_count {
                let module_idx = self.process_groups[group].members[mi];

                // Zero connected input ports at frame 0, then sum routes.
                let port_count = self.connected_in_ports[module_idx].len();
                for pi in 0..port_count {
                    let port = self.connected_in_ports[module_idx][pi];
                    if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                        inst.module_mut().input_block_mut(port)[0] = 0.0;
                    }
                }

                let route_count = self.compiled_routes[module_idx].len();
                for r in 0..route_count {
                    let route = self.compiled_routes[module_idx][r];
                    let value = if route.delayed {
                        if s == 0 {
                            self.out_prev[route.from_module][route.from_port]
                        } else {
                            self.out_bufs[route.from_module]
                                [route.from_port * self.block_capacity + (s - 1)]
                        }
                    } else {
                        self.out_bufs[route.from_module]
                            [route.from_port * self.block_capacity + s]
                    };
                    if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                        inst.module_mut().input_block_mut(route.to_port)[0] += value;
                    }
                }

                if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                    inst.module_mut().process(1);
                }

                let n_out = self.out_counts[module_idx];
                for p in 0..n_out {
                    let value = self
                        .modules
                        .get_index(module_idx)
                        .map(|(_, inst)| inst.module().output_block(p)[0])
                        .unwrap_or(0.0);
                    self.out_bufs[module_idx][p * self.block_capacity + s] = value;
                }
            }
        }
    }

    /// Records the final sample of each module output port for the next block's
    /// frame-0 delayed feedback reads.
    fn store_carry(&mut self, frames: usize) {
        let n = self.modules.len();
        for m in 0..n {
            let n_out = self.out_counts[m];
            for p in 0..n_out {
                self.out_prev[m][p] = self.out_bufs[m][p * self.block_capacity + (frames - 1)];
            }
        }
    }

    /// Drains all pending commands from the main thread and applies them.
    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.command_rx.try_recv() {
            self.apply_command(cmd);
        }
    }

    /// Applies a single command to the graph.
    pub(crate) fn apply_command(&mut self, cmd: GraphCommand) {
        match cmd {
            GraphCommand::SetModuleInput {
                module_id,
                port,
                value,
            } => {
                if let Some(module) = self.modules.get_mut(&module_id) {
                    let _ = module.module_mut().set_input(&port, value);
                }
            }
            GraphCommand::AddModule { module_id, module } => {
                let is_sink = matches!(module, GraphModule::Sink(_));
                self.modules.insert(module_id.clone(), module);
                if is_sink && !self.sinks.contains(&module_id) {
                    self.sinks.push(module_id);
                }
                self.topo_dirty = true;
            }
            GraphCommand::RemoveModule { module_id } => {
                self.modules.swap_remove(&module_id);
                self.sinks.retain(|id| id != &module_id);
                self.edges
                    .retain(|e| e.from_module != module_id && e.to_module != module_id);
                self.topo_dirty = true;
            }
            GraphCommand::AddConnection {
                from_module,
                from_port,
                to_module,
                to_port,
            } => {
                self.edges.push(RoutingConnection {
                    from_module,
                    from_port,
                    to_module,
                    to_port,
                });
                self.topo_dirty = true;
            }
            GraphCommand::RemoveConnection {
                from_module,
                from_port,
                to_module,
                to_port,
            } => {
                self.edges.retain(|e| {
                    !(e.from_module == from_module
                        && e.from_port == from_port
                        && e.to_module == to_module
                        && e.to_port == to_port)
                });
                self.topo_dirty = true;
            }
        }
    }

    /// Recomputes topological order, SCC process groups, compiled routes,
    /// input connectivity, output buffers, and sink indices.
    ///
    /// Called only when topology changes, never on the audio hot path.
    fn recompile(&mut self) {
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
        self.out_bufs = self.out_counts.iter().map(|&c| vec![0.0; c * cap]).collect();
        self.out_prev = self.out_counts.iter().map(|&c| vec![0.0; c]).collect();

        // Cache sink indices.
        self.sink_indices = self
            .sinks
            .iter()
            .filter_map(|id| self.modules.get_index_of(id.as_str()))
            .collect();
    }

    /// Returns the current processing order as module names (for testing).
    #[cfg(test)]
    fn process_order_names(&self) -> Vec<String> {
        self.process_order
            .iter()
            .filter_map(|&idx| self.modules.get_index(idx).map(|(k, _)| k.clone()))
            .collect()
    }
}

/// Computes strongly-connected components of `adj` (downstream adjacency) via
/// an iterative Tarjan's algorithm.
///
/// Returns `(comp_id, comps)` where `comp_id[node]` is the node's component
/// index and `comps` lists each component's members in **topological order**
/// (sources before sinks).
fn tarjan_scc(adj: &[Vec<usize>], n: usize) -> (Vec<usize>, Vec<Vec<usize>>) {
    const UNVISITED: usize = usize::MAX;

    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut comp_id = vec![UNVISITED; n];
    let mut scc_stack: Vec<usize> = Vec::new();
    let mut comps: Vec<Vec<usize>> = Vec::new();
    let mut next_index = 0usize;

    for start in 0..n {
        if index[start] != UNVISITED {
            continue;
        }

        // Explicit call stack of (node, next-child-cursor).
        let mut call_stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some((v, cursor)) = call_stack.last_mut() {
            let v = *v;
            if *cursor == 0 {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                scc_stack.push(v);
                on_stack[v] = true;
            }

            if *cursor < adj[v].len() {
                let w = adj[v][*cursor];
                *cursor += 1;
                if index[w] == UNVISITED {
                    call_stack.push((w, 0));
                } else if on_stack[w] && index[w] < low[v] {
                    low[v] = index[w];
                }
            } else {
                // Finished exploring v's children.
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = scc_stack.pop().unwrap();
                        on_stack[w] = false;
                        comp_id[w] = comps.len();
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    comps.push(comp);
                }
                call_stack.pop();
                if let Some((parent, _)) = call_stack.last() {
                    let parent = *parent;
                    if low[v] < low[parent] {
                        low[parent] = low[v];
                    }
                }
            }
        }
    }

    // Tarjan finalizes SCCs in reverse topological order; flip to topological.
    comps.reverse();
    let total = comps.len();
    for id in comp_id.iter_mut() {
        if *id != UNVISITED {
            *id = total - 1 - *id;
        }
    }

    (comp_id, comps)
}

#[cfg(test)]
mod tests;
