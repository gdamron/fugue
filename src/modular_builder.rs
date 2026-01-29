//! Modular patch builder using named port routing.
//!
//! This is the new builder system that replaces type-based routing with
//! explicit port-name routing for maximum flexibility.
//!
//! # Architecture Overview
//!
//! ## Signal Routing
//!
//! Unlike traditional type-based routing, this system uses **named ports** for connections:
//! - Each module declares its inputs/outputs via the `ModularModule` trait
//! - Connections specify port names: `{"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"}`
//! - All signals are f32 values - modules interpret them based on which port receives them
//!
//! ## Processing Order
//!
//! The system builds a dependency graph and processes modules in the correct order each sample:
//!
//! 1. **Reset all inputs** - Clear inputs to default state (crucial for gate/trigger signals)
//! 2. **Process source modules** - Modules with no inputs (Clock, Oscillator, etc.)
//! 3. **Iteratively route and process** - Process modules when all their inputs are ready
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
//!
//! ## Reset Behavior
//!
//! The `reset_inputs()` call before each sample is essential:
//!
//! - **Control signals** (gates, triggers, CV) must reset to default values
//! - **Parameters** (ADSR attack/decay, oscillator frequency) should NOT reset
//! - Example: ADSR gate must reset to 0.0, otherwise old gate values persist incorrectly
//! - This ensures clean state while preserving module configuration

use crate::module::{ModularModule, Module};
use crate::patch::Patch;
use crate::scale::{Mode, Note, Scale};
use crate::sequencer::MelodyParams;
use crate::synthesis::{ModularAdsr, Vca};
use crate::time::{Clock, Tempo};
use crate::{MelodyGenerator, OscillatorType};
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

/// Modular patch builder that uses named port routing.
///
/// Modules are connected via explicit port names rather than type-based routing.
pub struct ModularPatchBuilder {
    sample_rate: u32,
}

impl ModularPatchBuilder {
    /// Creates a new modular patch builder.
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Builds and prepares a modular patch for execution.
    pub fn build(&self, patch: Patch) -> Result<ModularPatchRuntime, Box<dyn std::error::Error>> {
        self.validate_patch(&patch)?;

        // Build all module instances
        let modules = self.build_modules(&patch)?;

        // Build the routing graph
        let routing = self.build_routing(&patch, &modules)?;

        // Find the DAC module
        let dac_id = patch
            .modules
            .iter()
            .find(|m| m.module_type == "dac")
            .ok_or("No DAC module found")?
            .id
            .clone();

        // Extract runtime controls (tempo, melody params)
        let tempo = modules
            .get("clock")
            .and_then(|m| {
                if let ModuleInstance::Clock(clock) = m {
                    Some(clock.lock().unwrap().tempo().clone())
                } else {
                    None
                }
            })
            .ok_or("No clock found for tempo control")?;

        let melody_params: Vec<MelodyParams> = modules
            .iter()
            .filter_map(|(_, m)| {
                if let ModuleInstance::Melody(melody) = m {
                    Some(melody.lock().unwrap().params().clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(ModularPatchRuntime {
            modules,
            routing,
            dac_id,
            tempo,
            melody_params,
        })
    }

    fn validate_patch(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Check all connections reference valid modules
        let module_ids: std::collections::HashSet<String> =
            patch.modules.iter().map(|m| m.id.clone()).collect();

        for conn in &patch.connections {
            if !module_ids.contains(&conn.from) {
                return Err(format!("Unknown source module: {}", conn.from).into());
            }
            if !module_ids.contains(&conn.to) {
                return Err(format!("Unknown destination module: {}", conn.to).into());
            }

            // Port names are required in modular system
            if conn.from_port.is_none() {
                return Err(format!("Missing from_port in connection from {}", conn.from).into());
            }
            if conn.to_port.is_none() {
                return Err(format!("Missing to_port in connection to {}", conn.to).into());
            }
        }

        // Check for DAC
        if !patch.modules.iter().any(|m| m.module_type == "dac") {
            return Err("No DAC module found".into());
        }

        // Check for cycles in the dependency graph
        self.validate_acyclic(patch)?;

        Ok(())
    }

    /// Validates that the patch contains no cycles (feedback loops).
    ///
    /// Uses depth-first search with a recursion stack to detect cycles.
    /// Cycles would cause infinite recursion in the pull-based system.
    fn validate_acyclic(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Build adjacency list (module -> modules it connects to)
        let mut graph: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for conn in &patch.connections {
            // Don't include DAC in cycle detection (it's a sink)
            if conn.to != "dac" {
                graph
                    .entry(conn.from.clone())
                    .or_insert_with(Vec::new)
                    .push(conn.to.clone());
            }
        }

        // Check each module for cycles using DFS
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();

        for module in &patch.modules {
            if module.id != "dac" && !visited.contains(&module.id) {
                if self.has_cycle_dfs(&module.id, &graph, &mut visited, &mut rec_stack) {
                    return Err(format!(
                        "Cycle detected in signal graph involving module '{}'",
                        module.id
                    )
                    .into());
                }
            }
        }

        Ok(())
    }

    /// Depth-first search to detect cycles.
    ///
    /// Returns true if a cycle is detected starting from `node`.
    fn has_cycle_dfs(
        &self,
        node: &str,
        graph: &std::collections::HashMap<String, Vec<String>>,
        visited: &mut std::collections::HashSet<String>,
        rec_stack: &mut std::collections::HashSet<String>,
    ) -> bool {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        // Check all neighbors
        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if self.has_cycle_dfs(neighbor, graph, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(neighbor) {
                    // Found a back edge - cycle detected!
                    return true;
                }
            }
        }

        rec_stack.remove(node);
        false
    }

    fn build_modules(
        &self,
        patch: &Patch,
    ) -> Result<IndexMap<String, ModuleInstance>, Box<dyn std::error::Error>> {
        let mut modules = IndexMap::new();

        for spec in &patch.modules {
            let module = match spec.module_type.as_str() {
                "clock" => {
                    let tempo = Tempo::new(spec.config.bpm.unwrap_or(120.0));
                    let mut clock = Clock::new(self.sample_rate, tempo).with_time_signature(
                        spec.config
                            .time_signature
                            .as_ref()
                            .map(|ts| ts.beats_per_measure)
                            .unwrap_or(4),
                    );

                    // Apply gate_duration if specified
                    if let Some(gate_dur) = spec.config.gate_duration {
                        clock = clock.with_gate_duration(gate_dur as f64);
                    }

                    ModuleInstance::Clock(Arc::new(Mutex::new(clock)))
                }
                "melody" => {
                    let root = Note::new(spec.config.root_note.unwrap_or(60));
                    let mode = self.parse_mode(spec.config.mode.as_deref().unwrap_or("dorian"))?;
                    let scale = Scale::new(root, mode);

                    let degrees = spec
                        .config
                        .scale_degrees
                        .clone()
                        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6]);
                    let params = MelodyParams::new(degrees);

                    if let Some(weights) = &spec.config.note_weights {
                        params.set_note_weights(weights.clone());
                    }

                    let melody = MelodyGenerator::new(scale, params);
                    ModuleInstance::Melody(Arc::new(Mutex::new(melody)))
                }
                "oscillator" => {
                    let osc_type = self.parse_oscillator_type(&spec.config)?;
                    let mut osc = crate::Oscillator::new(self.sample_rate, osc_type);

                    if let Some(freq) = spec.config.frequency {
                        osc.set_frequency(freq);
                    }
                    if let Some(fm) = spec.config.fm_amount {
                        osc.set_fm_amount(fm);
                    }
                    if let Some(am) = spec.config.am_amount {
                        osc.set_am_amount(am);
                    }

                    ModuleInstance::Oscillator(Arc::new(Mutex::new(osc)))
                }
                "adsr" => {
                    let mut adsr = ModularAdsr::new(self.sample_rate);

                    // Apply config values if specified
                    if let Some(attack) = spec.config.attack {
                        let _ = adsr.set_input("attack", attack);
                    }
                    if let Some(decay) = spec.config.decay {
                        let _ = adsr.set_input("decay", decay);
                    }
                    if let Some(sustain) = spec.config.sustain {
                        let _ = adsr.set_input("sustain", sustain);
                    }
                    if let Some(release) = spec.config.release {
                        let _ = adsr.set_input("release", release);
                    }

                    ModuleInstance::Adsr(Arc::new(Mutex::new(adsr)))
                }
                "vca" => {
                    let vca = Vca::new();
                    ModuleInstance::Vca(Arc::new(Mutex::new(vca)))
                }
                "dac" => ModuleInstance::Dac, // DAC is handled specially
                _ => {
                    return Err(format!("Unknown module type: {}", spec.module_type).into());
                }
            };

            modules.insert(spec.id.clone(), module);
        }

        Ok(modules)
    }

    fn build_routing(
        &self,
        patch: &Patch,
        modules: &IndexMap<String, ModuleInstance>,
    ) -> Result<Vec<RoutingConnection>, Box<dyn std::error::Error>> {
        let mut routing = Vec::new();

        for conn in &patch.connections {
            let from_port = conn.from_port.as_ref().ok_or("Missing from_port")?.clone();
            let to_port = conn.to_port.as_ref().ok_or("Missing to_port")?.clone();

            // Validate ports exist on modules
            if let Some(module) = modules.get(&conn.from) {
                module.validate_output_port(&from_port)?;
            }
            if let Some(module) = modules.get(&conn.to) {
                if conn.to != "dac" {
                    module.validate_input_port(&to_port)?;
                }
            }

            routing.push(RoutingConnection {
                from_module: conn.from.clone(),
                from_port,
                to_module: conn.to.clone(),
                to_port,
            });
        }

        Ok(routing)
    }

    fn parse_mode(&self, mode_str: &str) -> Result<Mode, Box<dyn std::error::Error>> {
        match mode_str.to_lowercase().as_str() {
            "ionian" | "major" => Ok(Mode::Ionian),
            "dorian" => Ok(Mode::Dorian),
            "phrygian" => Ok(Mode::Phrygian),
            "lydian" => Ok(Mode::Lydian),
            "mixolydian" => Ok(Mode::Mixolydian),
            "aeolian" | "minor" => Ok(Mode::Aeolian),
            "locrian" => Ok(Mode::Locrian),
            _ => Err(format!("Unknown mode: {}", mode_str).into()),
        }
    }

    fn parse_oscillator_type(
        &self,
        config: &crate::patch::ModuleConfig,
    ) -> Result<OscillatorType, Box<dyn std::error::Error>> {
        let osc_str = config.oscillator_type.as_deref().unwrap_or("sine");
        match osc_str.to_lowercase().as_str() {
            "sine" => Ok(OscillatorType::Sine),
            "square" => Ok(OscillatorType::Square),
            "sawtooth" | "saw" => Ok(OscillatorType::Sawtooth),
            "triangle" | "tri" => Ok(OscillatorType::Triangle),
            _ => Err(format!("Unknown oscillator type: {}", osc_str).into()),
        }
    }
}

/// A single routing connection in the signal graph.
#[derive(Debug, Clone)]
struct RoutingConnection {
    from_module: String,
    from_port: String,
    to_module: String,
    to_port: String,
}

/// Runtime instance of a modular module.
enum ModuleInstance {
    Clock(Arc<Mutex<Clock>>),
    Melody(Arc<Mutex<MelodyGenerator>>),
    Oscillator(Arc<Mutex<crate::Oscillator>>),
    Adsr(Arc<Mutex<ModularAdsr>>),
    Vca(Arc<Mutex<Vca>>),
    Dac, // Special case
}

impl ModuleInstance {
    fn as_modular_module(&self) -> Option<Arc<Mutex<dyn ModularModule + Send>>> {
        match self {
            ModuleInstance::Clock(m) => Some(m.clone() as Arc<Mutex<dyn ModularModule + Send>>),
            ModuleInstance::Melody(m) => Some(m.clone() as Arc<Mutex<dyn ModularModule + Send>>),
            ModuleInstance::Oscillator(m) => {
                Some(m.clone() as Arc<Mutex<dyn ModularModule + Send>>)
            }
            ModuleInstance::Adsr(m) => Some(m.clone() as Arc<Mutex<dyn ModularModule + Send>>),
            ModuleInstance::Vca(m) => Some(m.clone() as Arc<Mutex<dyn ModularModule + Send>>),
            ModuleInstance::Dac => None,
        }
    }

    fn validate_output_port(&self, port: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(module) = self.as_modular_module() {
            let m = module.lock().unwrap();
            if !m.outputs().contains(&port) {
                return Err(format!(
                    "Module does not have output port '{}'. Available: {:?}",
                    port,
                    m.outputs()
                )
                .into());
            }
        }
        Ok(())
    }

    fn validate_input_port(&self, port: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(module) = self.as_modular_module() {
            let m = module.lock().unwrap();
            if !m.inputs().contains(&port) {
                return Err(format!(
                    "Module does not have input port '{}'. Available: {:?}",
                    port,
                    m.inputs()
                )
                .into());
            }
        }
        Ok(())
    }
}

/// A prepared modular patch ready to run.
pub struct ModularPatchRuntime {
    modules: IndexMap<String, ModuleInstance>,
    routing: Vec<RoutingConnection>,
    dac_id: String,
    tempo: Tempo,
    melody_params: Vec<MelodyParams>,
}

impl ModularPatchRuntime {
    /// Starts audio playback.
    pub fn start(self) -> Result<RunningModularPatch, Box<dyn std::error::Error>> {
        // Build input map: for each module, collect all connections that feed into it
        let mut input_map: std::collections::HashMap<String, Vec<RoutingConnection>> =
            std::collections::HashMap::new();

        for conn in &self.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_insert_with(Vec::new)
                .push(conn.clone());
        }

        let graph = ModularSignalGraph {
            modules: self.modules,
            routing: self.routing,
            dac_id: self.dac_id,
            input_map,
            current_sample: 0,
        };

        let graph_arc = Arc::new(Mutex::new(graph));
        let generator = ModularGraphGenerator {
            graph: graph_arc.clone(),
        };

        let mut dac = crate::Dac::new()?;
        dac.start(generator)?;

        Ok(RunningModularPatch {
            dac,
            tempo: self.tempo,
            melody_params: self.melody_params,
            _graph: graph_arc,
        })
    }
}

/// The signal processing graph for modular routing.
struct ModularSignalGraph {
    modules: IndexMap<String, ModuleInstance>,
    routing: Vec<RoutingConnection>,
    dac_id: String,
    /// Maps module_id -> Vec of connections that feed into it (for pull-based processing)
    input_map: std::collections::HashMap<String, Vec<RoutingConnection>>,
    /// Current sample number (for caching)
    current_sample: u64,
}

impl ModularSignalGraph {
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
            if let Some(m) = module.as_modular_module() {
                let m_locked = m.lock().unwrap();

                // Cache hit - return cached value
                if m_locked.last_processed_sample() == self.current_sample {
                    return m_locked.get_cached_output(port);
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
                        if let Some(m) = to_module.as_modular_module() {
                            let _ = m.lock().unwrap().set_input(&conn.to_port, input_value);
                        }
                    }
                }

                // Now process the module
                if let Some(module) = self.modules.get(module_id) {
                    if let Some(m) = module.as_modular_module() {
                        let mut m_locked = m.lock().unwrap();
                        m_locked.process();
                        m_locked.mark_processed(self.current_sample);

                        // Return the requested output
                        return m_locked.get_cached_output(port);
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
    fn process_sample(&mut self) -> f32 {
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

/// Generator adapter for ModularSignalGraph.
struct ModularGraphGenerator {
    graph: Arc<Mutex<ModularSignalGraph>>,
}

impl Module for ModularGraphGenerator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "ModularGraphGenerator"
    }
}

impl crate::module::Generator<crate::Audio> for ModularGraphGenerator {
    fn output(&mut self) -> crate::Audio {
        let mut graph = self.graph.lock().unwrap();
        crate::Audio::new(graph.process_sample())
    }
}

/// A running modular patch with audio output.
pub struct RunningModularPatch {
    dac: crate::Dac,
    tempo: Tempo,
    melody_params: Vec<MelodyParams>,
    _graph: Arc<Mutex<ModularSignalGraph>>,
}

impl RunningModularPatch {
    /// Stops audio playback.
    pub fn stop(mut self) {
        self.dac.stop();
    }

    /// Returns the tempo controller.
    pub fn tempo(&self) -> &Tempo {
        &self.tempo
    }

    /// Returns the melody parameters for the first voice.
    pub fn melody_params(&self) -> &MelodyParams {
        &self.melody_params[0]
    }

    /// Returns all melody parameters.
    pub fn all_melody_params(&self) -> &[MelodyParams] {
        &self.melody_params
    }
}
