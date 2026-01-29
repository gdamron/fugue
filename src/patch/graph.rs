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
//! The system uses a **pull-based** approach where the DAC recursively requests inputs:
//!
//! 1. **DAC requests inputs** - For each sample, the DAC pulls from connected modules
//! 2. **Recursive dependency resolution** - Each module recursively pulls its inputs
//! 3. **Caching** - Modules cache outputs per sample to avoid reprocessing
//! 4. **Mix to DAC** - Combine all DAC inputs and output the final sample
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

use super::runtime::ModuleInstance;

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
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    pub(crate) routing: Vec<RoutingConnection>,
    pub(crate) dac_id: String,
    /// Maps module_id -> Vec of connections that feed into it (for pull-based processing)
    pub(crate) input_map: std::collections::HashMap<String, Vec<RoutingConnection>>,
    /// Current sample number (for caching)
    pub(crate) current_sample: u64,
}

impl SignalGraph {
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
            if let Some(m) = module.as_module() {
                let m_locked = m.lock().unwrap();

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
                        if let Some(m) = to_module.as_module() {
                            let _ = m.lock().unwrap().set_input(&conn.to_port, input_value);
                        }
                    }
                }

                // Now process the module
                if let Some(module) = self.modules.get(module_id) {
                    if let Some(m) = module.as_module() {
                        let mut m_locked = m.lock().unwrap();
                        m_locked.process();
                        m_locked.mark_processed(self.current_sample);

                        // Return the requested output
                        return m_locked.get_output(port);
                    }
                }
            }
        }

        Err(format!("Module '{}' not found", module_id))
    }

    /// Processes one sample through the entire graph.
    ///
    /// # Pull-Based Processing Algorithm
    ///
    /// 1. **Increment sample counter**: Track which sample we're processing
    /// 2. **Find DAC connections**: Determine what signals feed into the DAC
    /// 3. **Pull outputs**: Recursively request each DAC input (triggers dependency chain)
    /// 4. **Mix signals**: Combine all DAC inputs with gain compensation
    /// 5. **Return sample**: Output the final mixed audio sample
    ///
    /// The pull-based approach ensures correct processing order through recursive
    /// dependency resolution. Each module processes exactly once per sample via caching.
    pub(crate) fn process_sample(&mut self) -> f32 {
        // Increment sample counter for cache invalidation
        self.current_sample += 1;

        // Find all connections going to DAC
        let dac_connections: Vec<RoutingConnection> = self
            .routing
            .iter()
            .filter(|conn| conn.to_module == self.dac_id)
            .cloned()
            .collect();

        // Pull each DAC input (triggers recursive processing)
        let mut dac_signals = Vec::new();
        for conn in &dac_connections {
            match self.pull_output(&conn.from_module, &conn.from_port) {
                Ok(value) => dac_signals.push(value),
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to pull DAC input from {}:{} - {}",
                        conn.from_module, conn.from_port, e
                    );
                }
            }
        }

        // Mix DAC inputs with gain compensation
        if dac_signals.is_empty() {
            0.0
        } else if dac_signals.len() == 1 {
            dac_signals[0]
        } else {
            // Use sqrt(N) gain compensation to prevent clipping
            let gain = 1.0 / (dac_signals.len() as f32).sqrt();
            dac_signals.iter().sum::<f32>() * gain
        }
    }
}
