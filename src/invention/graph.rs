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

use crate::{GraphModule, SinkOutput};

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
#[derive(Debug, Clone)]
pub(crate) struct CompiledRoute {
    pub(crate) from_module: usize,
    pub(crate) from_port: usize,
    pub(crate) to_port: usize,
}

/// The signal processing graph for modular routing.
pub(crate) struct SignalGraph {
    /// All modules in the graph (including sinks).
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Sink modules that drive processing and collect output (by module id).
    pub(crate) sinks: Vec<String>,
    /// Authoritative edge list — updated by topology-change commands.
    pub(crate) edges: Vec<RoutingConnection>,
    /// Current sample number (for caching)
    pub(crate) current_sample: u64,
    /// Receiver for commands from the main thread.
    pub(crate) command_rx: mpsc::Receiver<GraphCommand>,
    /// Pre-computed topological processing order as module indices.
    pub(crate) process_order: Vec<usize>,
    /// Pre-compiled routes indexed by destination module index.
    /// `compiled_routes[i]` is the list of edges feeding the i-th module.
    pub(crate) compiled_routes: Vec<Vec<CompiledRoute>>,
    /// Sink module indices, cached for the hot path.
    pub(crate) sink_indices: Vec<usize>,
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

    pub(crate) fn process_modules(&mut self) {
        self.ensure_process_order();

        self.current_sample += 1;

        for module in self.modules.values_mut() {
            module.module_mut().reset_inputs();
        }

        for i in 0..self.process_order.len() {
            let module_idx = self.process_order[i];

            // Apply all input routes feeding this module.
            let route_count = self
                .compiled_routes
                .get(module_idx)
                .map(|r| r.len())
                .unwrap_or(0);
            for r in 0..route_count {
                let (from_idx, from_port, to_port) = {
                    let route = &self.compiled_routes[module_idx][r];
                    (route.from_module, route.from_port, route.to_port)
                };

                let input_value = self
                    .modules
                    .get_index(from_idx)
                    .map(|(_, m)| m.module().get_output_by_index(from_port))
                    .unwrap_or(0.0);
                if let Some((_, module)) = self.modules.get_index_mut(module_idx) {
                    module.module_mut().set_input_by_index(to_port, input_value);
                }
            }

            if let Some((_, module)) = self.modules.get_index_mut(module_idx) {
                let m = module.module_mut();
                m.process();
                m.mark_processed(self.current_sample);
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

    /// Recomputes topological order, compiled routes, and sink indices.
    ///
    /// Called only when topology changes, never on the audio hot path.
    fn recompile(&mut self) {
        let n = self.modules.len();

        // Build adjacency using module indices
        let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
        for edge in &self.edges {
            let Some(from_idx) = self.modules.get_index_of(edge.from_module.as_str()) else {
                continue;
            };
            let Some(to_idx) = self.modules.get_index_of(edge.to_module.as_str()) else {
                continue;
            };
            downstream[from_idx].push(to_idx);
        }

        // Iterative DFS producing reverse post-order (topological order).
        // State: 0 = unvisited, 1 = on stack (in progress), 2 = finished.
        // Back-edges become implicit one-sample delay points.
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
                        1 => {
                            // Back-edge (cycle) — skip, one-sample delay
                        }
                        _ => {
                            // Already finished — skip
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
        self.process_order = order;

        // Rebuild per-destination compiled route lists. Resolve port names
        // to indices once here; the hot path does no string work.
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
            self.compiled_routes[to_idx].push(CompiledRoute {
                from_module: from_idx,
                from_port,
                to_port,
            });
        }

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

    /// Processes one sample through the entire graph — allocation-free.
    ///
    /// # Linear Processing Algorithm
    ///
    /// 1. **Drain commands**: Apply any pending topology/parameter changes
    /// 2. **Recompile**: If topology changed, run topo sort + route compilation (allocates, but rare)
    /// 3. **Increment sample counter**: Track which sample we're processing
    /// 4. **Reset inputs**: Clear input "active" flags on all modules
    /// 5. **Linear iteration**: Process modules in topological order, setting inputs
    ///    from already-computed upstream outputs via index-based routes
    /// 6. **Collect sink output**: Sum sink outputs with gain compensation
    /// 7. **Return sample**: Output the final mixed stereo frame
    ///
    /// The pre-computed topological order guarantees dependencies are processed
    /// before their dependents. Zero heap allocations per sample.
    pub(crate) fn process_sample(&mut self) -> SinkOutput {
        self.process_modules();

        let sink_count = self.sink_indices.len();
        if sink_count == 0 {
            return SinkOutput::default();
        }

        let mut output = SinkOutput::default();
        for &sink_idx in &self.sink_indices {
            if let Some((_, module)) = self.modules.get_index(sink_idx) {
                if let Some(frame) = module.sink_output() {
                    output.left += frame.left;
                    output.right += frame.right;
                }
            }
        }

        if sink_count > 1 {
            let gain = 1.0 / (sink_count as f32).sqrt();
            output.left *= gain;
            output.right *= gain;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Creates a minimal SignalGraph for testing process order.
    /// Modules are stub oscillators — we only care about the topology.
    fn test_graph(module_ids: &[&str], connections: &[(&str, &str)]) -> SignalGraph {
        let (_tx, rx) = mpsc::channel();

        let registry = crate::ModuleRegistry::default();
        let null_config = serde_json::Value::Null;

        let mut modules = IndexMap::new();
        for &id in module_ids {
            let result = registry
                .build("oscillator", 44100, &null_config)
                .expect("oscillator is a valid type");
            modules.insert(id.to_string(), result.module);
        }

        let edges: Vec<RoutingConnection> = connections
            .iter()
            .map(|&(from, to)| RoutingConnection {
                from_module: from.to_string(),
                from_port: "audio".to_string(),
                to_module: to.to_string(),
                to_port: "fm".to_string(),
            })
            .collect();

        let mut graph = SignalGraph {
            modules,
            sinks: Vec::new(),
            edges,
            current_sample: 0,
            command_rx: rx,
            process_order: Vec::new(),
            compiled_routes: Vec::new(),
            sink_indices: Vec::new(),
            topo_dirty: true,
        };
        graph.recompile();
        graph
    }

    /// Helper: returns the position of `id` in the process order.
    fn pos(graph: &SignalGraph, id: &str) -> usize {
        graph
            .process_order_names()
            .iter()
            .position(|s| s == id)
            .unwrap_or_else(|| panic!("{id} not found in process_order"))
    }

    #[test]
    fn test_cycle_downstream_ordering() {
        // osc1 ↔ osc2 (cycle), osc1 → dac
        // All three nodes must be visited (Kahn's would strand dac).
        // dac must appear after osc1 (its direct dependency).
        let graph = test_graph(
            &["osc1", "osc2", "dac"],
            &[("osc1", "osc2"), ("osc2", "osc1"), ("osc1", "dac")],
        );

        let order = graph.process_order_names();
        assert_eq!(order.len(), 3, "all modules must be visited");
        assert!(
            pos(&graph, "dac") > pos(&graph, "osc1"),
            "dac must come after its dependency osc1, got order: {:?}",
            order,
        );
    }

    #[test]
    fn test_linear_chain_ordering() {
        // a → b → c — strict dependency order
        let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
        assert_eq!(graph.process_order_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_three_node_cycle_all_visited() {
        // a → b → c → a (cycle) — all three must appear in process order
        let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
        let order = graph.process_order_names();
        assert_eq!(order.len(), 3);
        // a should come before b, b before c (back-edge is c→a)
        assert!(pos(&graph, "a") < pos(&graph, "b"), "order: {:?}", order);
        assert!(pos(&graph, "b") < pos(&graph, "c"), "order: {:?}", order);
    }
}
