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

mod compile;
mod process;
mod scc;

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
    /// Peak level of the mixed master output, folded in each block for an
    /// off-thread sampler to drain into `MeterLevel` events (FUG-239 #5).
    pub(crate) master_peak: crate::atomic::StereoPeak,
    /// Mono tap of the mixed master output for off-thread spectrum analysis.
    /// Inert until something subscribes, and never analysed here.
    pub(crate) master_spectrum: crate::spectrum::SpectrumTap,
}

impl SignalGraph {
    /// Creates a graph over `modules` and `edges` with empty derived state.
    /// The topology is compiled on the first processed block.
    pub(crate) fn new(
        modules: IndexMap<String, ModuleInstance>,
        sinks: Vec<String>,
        edges: Vec<RoutingConnection>,
        command_rx: mpsc::Receiver<GraphCommand>,
        master_peak: crate::atomic::StereoPeak,
        master_spectrum: crate::spectrum::SpectrumTap,
    ) -> Self {
        Self {
            modules,
            sinks,
            edges,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            compiled_routes: Vec::new(),
            connected_in_ports: Vec::new(),
            process_groups: Vec::new(),
            sink_indices: Vec::new(),
            out_bufs: Vec::new(),
            out_prev: Vec::new(),
            out_counts: Vec::new(),
            block_capacity: 0,
            block_size: crate::DEFAULT_BLOCK_SIZE,
            topo_dirty: true,
            master_peak,
            master_spectrum,
        }
    }

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

    /// Returns the current processing order as module names (for testing).
    #[cfg(test)]
    fn process_order_names(&self) -> Vec<String> {
        self.process_order
            .iter()
            .filter_map(|&idx| self.modules.get_index(idx).map(|(k, _)| k.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests;
