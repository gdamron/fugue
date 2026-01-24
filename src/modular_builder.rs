//! Modular patch builder using named port routing.
//!
//! This is the new builder system that replaces type-based routing with
//! explicit port-name routing for maximum flexibility.

use crate::module::{ModularModule, Module};
use crate::patch::Patch;
use crate::scale::{Mode, Note, Scale};
use crate::sequencer::MelodyParams;
use crate::synthesis::{ModularAdsr, Vca};
use crate::time::{Clock, Tempo};
use crate::{MelodyGenerator, OscillatorType};
use std::collections::HashMap;
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
        let module_ids: HashMap<String, ()> =
            patch.modules.iter().map(|m| (m.id.clone(), ())).collect();

        for conn in &patch.connections {
            if !module_ids.contains_key(&conn.from) {
                return Err(format!("Unknown source module: {}", conn.from).into());
            }
            if !module_ids.contains_key(&conn.to) {
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

        Ok(())
    }

    fn build_modules(
        &self,
        patch: &Patch,
    ) -> Result<HashMap<String, ModuleInstance>, Box<dyn std::error::Error>> {
        let mut modules = HashMap::new();

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
                    if let Some(duration) = spec.config.note_duration {
                        params.set_note_duration(duration);
                    }

                    // Get tempo from clock (must be built first)
                    let tempo = modules
                        .get("clock")
                        .and_then(|m| {
                            if let ModuleInstance::Clock(clock) = m {
                                Some(clock.lock().unwrap().tempo().clone())
                            } else {
                                None
                            }
                        })
                        .ok_or("Clock must be defined before melody generator")?;

                    let melody = MelodyGenerator::new(scale, params, self.sample_rate, tempo);
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
                    let adsr = ModularAdsr::new(self.sample_rate);
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
        modules: &HashMap<String, ModuleInstance>,
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
    modules: HashMap<String, ModuleInstance>,
    routing: Vec<RoutingConnection>,
    dac_id: String,
    tempo: Tempo,
    melody_params: Vec<MelodyParams>,
}

impl ModularPatchRuntime {
    /// Starts audio playback.
    pub fn start(self) -> Result<RunningModularPatch, Box<dyn std::error::Error>> {
        let graph = ModularSignalGraph {
            modules: self.modules,
            routing: self.routing,
            dac_id: self.dac_id,
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
    modules: HashMap<String, ModuleInstance>,
    routing: Vec<RoutingConnection>,
    dac_id: String,
}

impl ModularSignalGraph {
    /// Processes one sample through the entire graph.
    fn process_sample(&mut self) -> f32 {
        // Reset all module inputs
        for module in self.modules.values() {
            if let Some(m) = module.as_modular_module() {
                m.lock().unwrap().reset_inputs();
            }
        }

        // Process modules in dependency order by iterating through routing connections
        // For each connection: process source, route signal to destination
        // This ensures modules are processed before their outputs are needed

        // First pass: Process all source modules (those that appear as "from" but not as "to")
        let mut processed: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Find and process source modules (no inputs)
        let destinations: std::collections::HashSet<String> = self
            .routing
            .iter()
            .filter(|c| c.to_module != self.dac_id)
            .map(|c| c.to_module.clone())
            .collect();

        for (id, module) in &self.modules {
            if !destinations.contains(id) && id != &self.dac_id {
                // This is a source module
                if let Some(m) = module.as_modular_module() {
                    m.lock().unwrap().process();
                    processed.insert(id.clone());
                }
            }
        }

        // Now iteratively process modules whose inputs are ready
        let max_iterations = self.modules.len() * 2; // Prevent infinite loops
        for _ in 0..max_iterations {
            let mut made_progress = false;

            // Route signals from processed modules to unprocessed ones
            for conn in &self.routing {
                if processed.contains(&conn.from_module)
                    && !processed.contains(&conn.to_module)
                    && conn.to_module != self.dac_id
                {
                    // Get output from source
                    let value = if let Some(from_module) = self.modules.get(&conn.from_module) {
                        if let Some(m) = from_module.as_modular_module() {
                            m.lock().unwrap().get_output(&conn.from_port).unwrap_or(0.0)
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    // Set input on destination
                    if let Some(to_module) = self.modules.get(&conn.to_module) {
                        if let Some(m) = to_module.as_modular_module() {
                            let _ = m.lock().unwrap().set_input(&conn.to_port, value);
                        }
                    }
                }
            }

            // Process modules whose inputs have been set
            for (id, module) in &self.modules {
                if !processed.contains(id) && id != &self.dac_id {
                    // Check if all inputs for this module have been provided
                    let inputs_ready = self
                        .routing
                        .iter()
                        .filter(|c| &c.to_module == id)
                        .all(|c| processed.contains(&c.from_module));

                    if inputs_ready {
                        if let Some(m) = module.as_modular_module() {
                            m.lock().unwrap().process();
                            processed.insert(id.clone());
                            made_progress = true;
                        }
                    }
                }
            }

            if !made_progress {
                break;
            }
        }

        // Finally, collect and mix all signals going to DAC
        let mut dac_signals = Vec::new();
        for conn in &self.routing {
            if conn.to_module == self.dac_id {
                if let Some(from_module) = self.modules.get(&conn.from_module) {
                    if let Some(m) = from_module.as_modular_module() {
                        if let Ok(value) = m.lock().unwrap().get_output(&conn.from_port) {
                            dac_signals.push(value);
                        }
                    }
                }
            }
        }

        // Mix DAC inputs
        if dac_signals.is_empty() {
            0.0
        } else if dac_signals.len() == 1 {
            dac_signals[0]
        } else {
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
