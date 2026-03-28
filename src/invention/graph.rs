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
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::{SinkModule, SinkOutput};

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
        sink: Option<SinkInstance>,
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

/// Type alias for sink module instances.
pub(crate) type SinkInstance = Arc<Mutex<dyn SinkModule + Send>>;

/// A single routing connection in the signal graph.
#[derive(Debug, Clone)]
pub(crate) struct RoutingConnection {
    pub(crate) from_module: String,
    pub(crate) from_port: String,
    pub(crate) to_module: String,
    pub(crate) to_port: String,
}

/// The signal processing graph for modular routing.
pub(crate) struct SignalGraph {
    /// All modules in the graph (including sinks).
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Sink modules that drive processing and collect output.
    pub(crate) sinks: IndexMap<String, SinkInstance>,
    /// Maps module_id -> Vec of connections that feed into it
    pub(crate) input_map: HashMap<String, Vec<RoutingConnection>>,
    /// Current sample number (for caching)
    pub(crate) current_sample: u64,
    /// Receiver for commands from the main thread.
    pub(crate) command_rx: mpsc::Receiver<GraphCommand>,
    /// Pre-computed topological processing order.
    pub(crate) process_order: Vec<String>,
    /// Flag indicating topology changed and process_order needs recomputation.
    pub(crate) topo_dirty: bool,
}

impl SignalGraph {
    /// Drains all pending commands from the main thread and applies them.
    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.command_rx.try_recv() {
            self.apply_command(cmd);
        }
    }

    /// Applies a single command to the graph.
    fn apply_command(&mut self, cmd: GraphCommand) {
        match cmd {
            GraphCommand::SetModuleInput {
                module_id,
                port,
                value,
            } => {
                if let Some(module) = self.modules.get(&module_id) {
                    let _ = module.lock().unwrap().set_input(&port, value);
                }
            }
            GraphCommand::AddModule {
                module_id,
                module,
                sink,
            } => {
                self.modules.insert(module_id.clone(), module);
                if let Some(sink) = sink {
                    self.sinks.insert(module_id, sink);
                }
                self.topo_dirty = true;
            }
            GraphCommand::RemoveModule { module_id } => {
                self.modules.swap_remove(&module_id);
                self.sinks.swap_remove(&module_id);
                self.input_map.remove(&module_id);
                // Remove connections referencing this module from all other input maps
                for connections in self.input_map.values_mut() {
                    connections.retain(|conn| conn.from_module != module_id);
                }
                self.topo_dirty = true;
            }
            GraphCommand::AddConnection {
                from_module,
                from_port,
                to_module,
                to_port,
            } => {
                self.input_map
                    .entry(to_module.clone())
                    .or_default()
                    .push(RoutingConnection {
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
                if let Some(connections) = self.input_map.get_mut(&to_module) {
                    connections.retain(|conn| {
                        !(conn.from_module == from_module
                            && conn.from_port == from_port
                            && conn.to_port == to_port)
                    });
                }
                self.topo_dirty = true;
            }
        }
    }

    /// Recomputes the topological processing order using iterative DFS.
    ///
    /// Called only when topology changes (module/connection add/remove), never
    /// on the audio hot path. Produces a reverse post-order traversal which
    /// gives a valid topological ordering. Back-edges (cycles) are detected
    /// and skipped — those edges become one-sample delay points. Unlike Kahn's
    /// algorithm, DFS visits **all** nodes, so modules downstream of cycles
    /// are correctly ordered after their dependencies.
    fn recompute_process_order(&mut self) {
        let module_ids: Vec<String> = self.modules.keys().cloned().collect();

        // Build adjacency: module -> modules it feeds into (downstream)
        let mut downstream: HashMap<&str, Vec<&str>> = HashMap::new();
        for connections in self.input_map.values() {
            for conn in connections {
                if self.modules.contains_key(&conn.to_module)
                    && self.modules.contains_key(&conn.from_module)
                {
                    downstream
                        .entry(conn.from_module.as_str())
                        .or_default()
                        .push(conn.to_module.as_str());
                }
            }
        }

        // Iterative DFS producing reverse post-order (topological order)
        // State: 0 = unvisited, 1 = on stack (in progress), 2 = finished
        let mut state: HashMap<&str, u8> = HashMap::new();
        for id in &module_ids {
            state.insert(id.as_str(), 0);
        }

        let mut order: Vec<String> = Vec::with_capacity(module_ids.len());

        // Visit nodes in deterministic (IndexMap insertion) order
        for start in &module_ids {
            if state[start.as_str()] != 0 {
                continue;
            }

            // Iterative DFS using an explicit stack
            // Each entry: (node, index into its downstream neighbors)
            let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
            *state.get_mut(start.as_str()).unwrap() = 1; // on stack

            while let Some((node, idx)) = stack.last_mut() {
                let neighbors = downstream.get(*node).map(|v| v.as_slice()).unwrap_or(&[]);
                if *idx < neighbors.len() {
                    let next = neighbors[*idx];
                    *idx += 1;
                    match state.get(next) {
                        Some(0) => {
                            // Unvisited — push onto stack
                            *state.get_mut(next).unwrap() = 1;
                            stack.push((next, 0));
                        }
                        Some(1) => {
                            // Back-edge (cycle) — skip, this becomes
                            // the one-sample delay point
                        }
                        _ => {
                            // Already finished — skip (cross/forward edge)
                        }
                    }
                } else {
                    // All neighbors visited — finish this node (post-order)
                    let (finished, _) = stack.pop().unwrap();
                    *state.get_mut(finished).unwrap() = 2;
                    order.push(finished.to_string());
                }
            }
        }

        // Reverse post-order = topological order
        order.reverse();
        self.process_order = order;
    }

    /// Returns the current processing order (for testing).
    #[cfg(test)]
    fn process_order(&self) -> &[String] {
        &self.process_order
    }

    /// Processes one sample through the entire graph — allocation-free.
    ///
    /// # Linear Processing Algorithm
    ///
    /// 1. **Drain commands**: Apply any pending topology/parameter changes
    /// 2. **Recompute order**: If topology changed, run topo sort (allocates, but rare)
    /// 3. **Increment sample counter**: Track which sample we're processing
    /// 4. **Reset inputs**: Clear input "active" flags on all modules
    /// 5. **Linear iteration**: Process modules in topological order, setting inputs
    ///    from already-computed upstream outputs
    /// 6. **Collect sink output**: Sum sink outputs with gain compensation
    /// 7. **Return sample**: Output the final mixed stereo frame
    ///
    /// The pre-computed topological order guarantees dependencies are processed
    /// before their dependents. Zero heap allocations per sample.
    pub(crate) fn process_sample(&mut self) -> SinkOutput {
        self.drain_commands();

        if self.topo_dirty {
            self.recompute_process_order();
            self.topo_dirty = false;
        }

        self.current_sample += 1;

        // Reset input "active" flags on all modules
        for module in self.modules.values() {
            module.lock().unwrap().reset_inputs();
        }

        // Process modules in topological order
        for i in 0..self.process_order.len() {
            let module_id = &self.process_order[i];

            // Set inputs from already-processed upstream modules
            if let Some(connections) = self.input_map.get(module_id.as_str()) {
                for conn in connections {
                    let input_value = self
                        .modules
                        .get(&conn.from_module)
                        .map(|m| m.lock().unwrap().get_output(&conn.from_port).unwrap_or(0.0))
                        .unwrap_or(0.0);
                    if let Some(module) = self.modules.get(module_id.as_str()) {
                        let _ = module.lock().unwrap().set_input(&conn.to_port, input_value);
                    }
                }
            }

            // Process the module
            if let Some(module) = self.modules.get(module_id.as_str()) {
                let mut m = module.lock().unwrap();
                m.process();
                m.mark_processed(self.current_sample);
            }
        }

        // Collect sink outputs — no allocation, just accumulate
        let sink_count = self.sinks.len();
        if sink_count == 0 {
            return SinkOutput::default();
        }

        let mut output = SinkOutput::default();
        for sink in self.sinks.values() {
            let frame = sink.lock().unwrap().sink_output();
            output.left += frame.left;
            output.right += frame.right;
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

        let mut input_map: HashMap<String, Vec<RoutingConnection>> = HashMap::new();
        for &(from, to) in connections {
            input_map
                .entry(to.to_string())
                .or_default()
                .push(RoutingConnection {
                    from_module: from.to_string(),
                    from_port: "audio".to_string(),
                    to_module: to.to_string(),
                    to_port: "fm".to_string(),
                });
        }

        let mut graph = SignalGraph {
            modules,
            sinks: IndexMap::new(),
            input_map,
            current_sample: 0,
            command_rx: rx,
            process_order: Vec::new(),
            topo_dirty: true,
        };
        graph.recompute_process_order();
        graph
    }

    /// Helper: returns the position of `id` in the process order.
    fn pos(graph: &SignalGraph, id: &str) -> usize {
        graph
            .process_order()
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

        let order = graph.process_order();
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
        assert_eq!(graph.process_order(), &["a", "b", "c"]);
    }

    #[test]
    fn test_three_node_cycle_all_visited() {
        // a → b → c → a (cycle) — all three must appear in process order
        let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
        let order = graph.process_order();
        assert_eq!(order.len(), 3);
        // a should come before b, b before c (back-edge is c→a)
        assert!(pos(&graph, "a") < pos(&graph, "b"), "order: {:?}", order);
        assert!(pos(&graph, "b") < pos(&graph, "c"), "order: {:?}", order);
    }
}
