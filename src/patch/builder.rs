//! Patch builder for creating modular synthesis setups.

use crate::modules::{Adsr, Clock, MelodyGenerator, Oscillator, Tempo, Vca};
use crate::modules::{MelodyParams, OscillatorType};
use crate::music::{Mode, Note, Scale};
use crate::patch::format::{ModuleConfig, Patch};
use crate::patch::runtime::{ModuleInstance, PatchRuntime};
use crate::Module;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::graph::RoutingConnection;

/// Patch builder that uses named port routing.
///
/// Modules are connected via explicit port names rather than type-based routing.
pub struct PatchBuilder {
    sample_rate: u32,
}

impl PatchBuilder {
    /// Creates a new patch builder.
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Builds and prepares a patch for execution.
    pub fn build(&self, patch: Patch) -> Result<PatchRuntime, Box<dyn std::error::Error>> {
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

        Ok(PatchRuntime {
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
                    .or_default()
                    .push(conn.to.clone());
            }
        }

        // Check each module for cycles using DFS
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();

        for module in &patch.modules {
            if module.id != "dac"
                && !visited.contains(&module.id)
                && self.has_cycle_dfs(&module.id, &graph, &mut visited, &mut rec_stack)
            {
                return Err(format!(
                    "Cycle detected in signal graph involving module '{}'",
                    module.id
                )
                .into());
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
                    let mut osc = Oscillator::new(self.sample_rate, osc_type);

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
                    let mut adsr = Adsr::new(self.sample_rate);

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
        config: &ModuleConfig,
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
