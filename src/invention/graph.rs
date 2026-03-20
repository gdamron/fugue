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
//! 1. **Topo sort** - Kahn's algorithm determines dependency order (runs only on topology change)
//! 2. **Linear iteration** - Each sample iterates the sorted order, setting inputs from upstream outputs
//! 3. **No recursion** - All modules processed in a single pass with zero per-sample allocations
//! 4. **Cycle handling** - Modules in cycles are appended last; they read one-sample-delayed values
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

use crate::SinkModule;

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

    /// Recomputes the topological processing order using Kahn's algorithm.
    ///
    /// Called only when topology changes (module/connection add/remove), never
    /// on the audio hot path. Modules involved in cycles are appended at the
    /// end — they'll read one-sample-delayed values from upstream.
    fn recompute_process_order(&mut self) {
        let module_ids: Vec<String> = self.modules.keys().cloned().collect();

        // Compute in-degree for each module
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for id in &module_ids {
            in_degree.insert(id.as_str(), 0);
        }
        for connections in self.input_map.values() {
            for conn in connections {
                if self.modules.contains_key(&conn.to_module) && self.modules.contains_key(&conn.from_module) {
                    if let Some(deg) = in_degree.get_mut(conn.to_module.as_str()) {
                        *deg += 1;
                    }
                }
            }
        }

        // Build adjacency: module -> list of modules it feeds into
        let mut downstream: HashMap<&str, Vec<&str>> = HashMap::new();
        for connections in self.input_map.values() {
            for conn in connections {
                if self.modules.contains_key(&conn.to_module) && self.modules.contains_key(&conn.from_module) {
                    downstream
                        .entry(conn.from_module.as_str())
                        .or_default()
                        .push(conn.to_module.as_str());
                }
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<&str> = Vec::new();
        for (id, &deg) in &in_degree {
            if deg == 0 {
                queue.push(id);
            }
        }
        // Sort the initial queue for deterministic order
        queue.sort();

        let mut order: Vec<String> = Vec::with_capacity(module_ids.len());
        while let Some(id) = queue.pop() {
            order.push(id.to_string());
            if let Some(targets) = downstream.get(id) {
                for &target in targets {
                    if let Some(deg) = in_degree.get_mut(target) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(target);
                            // Keep queue sorted for deterministic processing
                            queue.sort();
                        }
                    }
                }
            }
        }

        // Append any remaining modules (involved in cycles) at the end
        for id in &module_ids {
            if !order.contains(id) {
                order.push(id.clone());
            }
        }

        self.process_order = order;
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
    /// 7. **Return sample**: Output the final mixed audio sample
    ///
    /// The pre-computed topological order guarantees dependencies are processed
    /// before their dependents. Zero heap allocations per sample.
    pub(crate) fn process_sample(&mut self) -> f32 {
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
            return 0.0;
        }

        let mut output = 0.0f32;
        for sink in self.sinks.values() {
            output += sink.lock().unwrap().sink_output().sample;
        }

        if sink_count > 1 {
            output *= 1.0 / (sink_count as f32).sqrt();
        }
        output
    }
}
