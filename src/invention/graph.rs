//! Signal processing graph for pull-based modular routing.
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
//! The system uses a **pull-based** approach where sink modules drive processing:
//!
//! 1. **Pull from sinks** - For each sample, pull from all sink module inputs
//! 2. **Recursive dependency resolution** - Each module recursively pulls its inputs
//! 3. **Caching** - Modules cache outputs per sample to avoid reprocessing
//! 4. **Process remaining** - Process any disconnected modules (for observable side effects)
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
    RemoveModule {
        module_id: String,
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
    /// Maps module_id -> Vec of connections that feed into it (for pull-based processing)
    pub(crate) input_map: std::collections::HashMap<String, Vec<RoutingConnection>>,
    /// Current sample number (for caching)
    pub(crate) current_sample: u64,
    /// Receiver for commands from the main thread.
    pub(crate) command_rx: mpsc::Receiver<GraphCommand>,
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
            }
            GraphCommand::RemoveModule { module_id } => {
                self.modules.swap_remove(&module_id);
                self.sinks.swap_remove(&module_id);
                self.input_map.remove(&module_id);
                // Remove connections referencing this module from all other input maps
                for connections in self.input_map.values_mut() {
                    connections.retain(|conn| conn.from_module != module_id);
                }
            }
        }
    }

    /// Pulls an output value from a module using recursive dependency resolution.
    ///
    /// # Algorithm
    ///
    /// 1. Check if module already processed this sample (cache hit)
    /// 2. If cached, return the cached output value
    /// 3. Get all input connections for this module
    /// 4. Recursively pull outputs from all dependencies
    /// 5. Set all inputs on the module
    /// 6. Process the module
    /// 7. Mark as processed for this sample
    /// 8. Return the requested output value
    ///
    /// This ensures correct processing order through depth-first traversal.
    fn pull_output(&mut self, module_id: &str, port: &str) -> Result<f32, String> {
        // Check if already processed this sample
        if let Some(module) = self.modules.get(module_id) {
            let m_locked = module.lock().unwrap();

            // Cache hit - return cached value
            if m_locked.last_processed_sample() == self.current_sample {
                return m_locked.get_output(port);
            }

            // Cache miss - need to process this module
            // First, recursively pull all inputs

            // Clone input connections to avoid borrow issues during recursion
            let input_connections: Vec<RoutingConnection> =
                self.input_map.get(module_id).cloned().unwrap_or_default();

            // Drop the lock before recursion
            drop(m_locked);

            // Recursively pull all inputs
            for conn in &input_connections {
                let input_value = self
                    .pull_output(&conn.from_module, &conn.from_port)
                    .unwrap_or_else(|e| {
                        eprintln!(
                            "Warning: Failed to pull {}:{} - {}",
                            conn.from_module, conn.from_port, e
                        );
                        0.0
                    });

                // Set the input on this module
                if let Some(to_module) = self.modules.get(module_id) {
                    let _ = to_module
                        .lock()
                        .unwrap()
                        .set_input(&conn.to_port, input_value);
                }
            }

            // Now process the module
            if let Some(module) = self.modules.get(module_id) {
                let mut m_locked = module.lock().unwrap();
                m_locked.process();
                m_locked.mark_processed(self.current_sample);

                // Return the requested output
                return m_locked.get_output(port);
            }
        }

        Err(format!("Module '{}' not found", module_id))
    }

    /// Pulls all inputs for a module without returning an output.
    ///
    /// Used for sink modules where we need to process dependencies
    /// but the output comes from `sink_output()` instead.
    fn pull_inputs_for_module(&mut self, module_id: &str) {
        // Clone input connections to avoid borrow issues during recursion
        let input_connections: Vec<RoutingConnection> =
            self.input_map.get(module_id).cloned().unwrap_or_default();

        // Recursively pull all inputs
        for conn in &input_connections {
            let input_value = self
                .pull_output(&conn.from_module, &conn.from_port)
                .unwrap_or_else(|e| {
                    eprintln!(
                        "Warning: Failed to pull {}:{} - {}",
                        conn.from_module, conn.from_port, e
                    );
                    0.0
                });

            // Set the input on this module
            if let Some(to_module) = self.modules.get(module_id) {
                let _ = to_module
                    .lock()
                    .unwrap()
                    .set_input(&conn.to_port, input_value);
            }
        }
    }

    /// Processes one sample through the entire graph.
    ///
    /// # Pull-Based Processing Algorithm
    ///
    /// 1. **Increment sample counter**: Track which sample we're processing
    /// 2. **Reset inputs**: Clear input "active" flags on all modules
    /// 3. **Pull from sinks**: For each sink module, pull its inputs (triggers dependency chain)
    /// 4. **Process sinks**: Process each sink and collect its output
    /// 5. **Process remaining**: Process any modules not yet processed (disconnected modules)
    /// 6. **Mix outputs**: Combine all sink outputs with gain compensation
    /// 7. **Return sample**: Output the final mixed audio sample
    ///
    /// The pull-based approach ensures correct processing order through recursive
    /// dependency resolution. Each module processes exactly once per sample via caching.
    pub(crate) fn process_sample(&mut self) -> f32 {
        // Drain any pending commands from the main thread
        self.drain_commands();

        // Increment sample counter for cache invalidation
        self.current_sample += 1;

        // Reset input "active" flags on all modules
        // This allows modules to distinguish between signal inputs and control defaults
        for (_module_id, module) in &self.modules {
            module.lock().unwrap().reset_inputs();
        }

        // Collect sink outputs
        let mut sink_outputs = Vec::new();

        // Clone sink IDs to avoid borrow issues
        let sink_ids: Vec<String> = self.sinks.keys().cloned().collect();

        for sink_id in &sink_ids {
            // Pull all inputs for this sink (triggers recursive processing)
            self.pull_inputs_for_module(sink_id);

            // Process the sink and collect output
            if let Some(sink) = self.sinks.get(sink_id) {
                let mut s = sink.lock().unwrap();
                s.process();
                s.mark_processed(self.current_sample);
                sink_outputs.push(s.sink_output().sample);
            }
        }

        // Process any modules not yet processed this sample
        // This handles disconnected modules that may have observable side effects
        for (_module_id, module) in &self.modules {
            let mut m = module.lock().unwrap();
            if m.last_processed_sample() != self.current_sample {
                m.process();
                m.mark_processed(self.current_sample);
            }
        }

        // Mix sink outputs with gain compensation
        Self::mix_outputs(&sink_outputs)
    }

    /// Mixes multiple audio signals with gain compensation.
    fn mix_outputs(outputs: &[f32]) -> f32 {
        if outputs.is_empty() {
            0.0
        } else if outputs.len() == 1 {
            outputs[0]
        } else {
            // Use sqrt(N) gain compensation to prevent clipping
            let gain = 1.0 / (outputs.len() as f32).sqrt();
            outputs.iter().sum::<f32>() * gain
        }
    }
}
