//! Patch builder for FM/AM oscillator synthesis patches.
//!
//! Handles patches containing only oscillators and a DAC (no clock, melody,
//! or voice modules). Used for direct frequency/amplitude modulation synthesis.

use crate::module::{Generator, Module};
use crate::patch::{ModuleConfig, ModuleSpec, Patch};
use crate::{Audio, Dac, ModulatedOscillator, ModulationInputs, OscillatorType};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Constructs runnable audio graphs from oscillator-only patches.
///
/// Use this builder for FM/AM synthesis patches where oscillators modulate
/// each other directly without clock-driven sequencing. For sequenced patches,
/// use [`PatchBuilder`](crate::PatchBuilder) instead.
pub struct OscillatorPatchBuilder {
    sample_rate: u32,
}

impl OscillatorPatchBuilder {
    /// Creates a new builder with the given sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Builds a runnable patch from an oscillator-only patch document.
    ///
    /// Parses the patch, creates oscillators with their modulation routing,
    /// and returns an [`OscillatorPatchRuntime`] ready to be started.
    pub fn build_and_run(
        &self,
        patch: Patch,
    ) -> Result<OscillatorPatchRuntime, Box<dyn std::error::Error>> {
        // Build oscillators
        let module_map: HashMap<String, &ModuleSpec> =
            patch.modules.iter().map(|m| (m.id.clone(), m)).collect();

        let mut oscillators: HashMap<String, Arc<Mutex<ModulatedOscillator>>> = HashMap::new();

        // Create all oscillators
        for module in &patch.modules {
            if module.module_type == "oscillator" {
                let osc = self.build_oscillator(&module.config)?;
                oscillators.insert(module.id.clone(), Arc::new(Mutex::new(osc)));
            }
        }

        // Build modulation routing
        let mut modulation_map: HashMap<String, ModulationConnections> = HashMap::new();

        for conn in &patch.connections {
            // Skip connections to DAC
            if module_map.get(&conn.to).map(|m| m.module_type.as_str()) == Some("dac") {
                continue;
            }

            if let Some(port) = &conn.to_port {
                let entry = modulation_map
                    .entry(conn.to.clone())
                    .or_insert_with(ModulationConnections::default);

                match port.as_str() {
                    "frequency" | "freq" | "fm" => entry.frequency_source = Some(conn.from.clone()),
                    "amplitude" | "amp" | "gain" | "am" => {
                        entry.amplitude_source = Some(conn.from.clone())
                    }
                    _ => return Err(format!("Unknown modulation port: {}", port).into()),
                }
            }
        }

        // Find output oscillator (the one connected to DAC)
        let output_id = patch
            .connections
            .iter()
            .find(|c| module_map.get(&c.to).map(|m| m.module_type.as_str()) == Some("dac"))
            .ok_or("No oscillator connected to DAC")?
            .from
            .clone();

        Ok(OscillatorPatchRuntime {
            patch,
            oscillators,
            modulation_map,
            output_id,
        })
    }

    /// Creates a modulated oscillator from a module configuration.
    fn build_oscillator(
        &self,
        config: &ModuleConfig,
    ) -> Result<ModulatedOscillator, Box<dyn std::error::Error>> {
        let osc_type = self.parse_oscillator_type(config)?;
        let frequency = config.frequency.unwrap_or(440.0);
        let fm_amount = config.fm_amount.unwrap_or(0.0);
        let am_amount = config.am_amount.unwrap_or(0.0);

        Ok(ModulatedOscillator::new(self.sample_rate, osc_type)
            .with_frequency(frequency)
            .with_fm_amount(fm_amount)
            .with_am_amount(am_amount))
    }

    /// Parses the oscillator type from a module configuration.
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

/// Tracks which oscillators modulate another oscillator's inputs.
#[derive(Default)]
struct ModulationConnections {
    /// ID of the oscillator providing frequency modulation, if any.
    frequency_source: Option<String>,
    /// ID of the oscillator providing amplitude modulation, if any.
    amplitude_source: Option<String>,
}

/// A validated oscillator patch ready to be started.
///
/// Created by [`OscillatorPatchBuilder::build_and_run`]. Call
/// [`start`](Self::start) to begin audio playback.
pub struct OscillatorPatchRuntime {
    patch: Patch,
    oscillators: HashMap<String, Arc<Mutex<ModulatedOscillator>>>,
    modulation_map: HashMap<String, ModulationConnections>,
    output_id: String,
}

impl OscillatorPatchRuntime {
    /// Starts audio playback and returns a handle to the running patch.
    pub fn start(self) -> Result<RunningOscillatorPatch, Box<dyn std::error::Error>> {
        let graph = OscillatorGraph {
            oscillators: self.oscillators,
            modulation_map: self.modulation_map,
            output_id: self.output_id,
        };

        let generator = OscillatorGraphGenerator {
            graph: Arc::new(Mutex::new(graph)),
        };

        let mut dac = Dac::new()?;
        dac.start(generator)?;

        Ok(RunningOscillatorPatch {
            patch: self.patch,
            dac,
        })
    }

    /// Returns a reference to the patch document.
    pub fn patch(&self) -> &Patch {
        &self.patch
    }
}

/// Internal graph of oscillators with modulation routing.
struct OscillatorGraph {
    oscillators: HashMap<String, Arc<Mutex<ModulatedOscillator>>>,
    modulation_map: HashMap<String, ModulationConnections>,
    output_id: String,
}

impl OscillatorGraph {
    /// Processes all oscillators and returns the output audio sample.
    fn process_and_output(&self) -> Audio {
        // First, process all oscillators that are modulators
        let mut osc_outputs: HashMap<String, f32> = HashMap::new();

        // Process each oscillator
        for (id, osc) in &self.oscillators {
            let mut osc_mut = osc.lock().unwrap();
            osc_mut.process();

            // Get modulation inputs for this oscillator
            let mod_inputs = if let Some(mod_conn) = self.modulation_map.get(id) {
                ModulationInputs {
                    frequency: mod_conn
                        .frequency_source
                        .as_ref()
                        .and_then(|src| osc_outputs.get(src).copied())
                        .unwrap_or(0.0),
                    amplitude: mod_conn
                        .amplitude_source
                        .as_ref()
                        .and_then(|src| osc_outputs.get(src).copied())
                        .unwrap_or(0.0),
                }
            } else {
                ModulationInputs::default()
            };

            // Generate output with modulation
            let output = osc_mut.process_with_modulation(mod_inputs);
            osc_outputs.insert(id.clone(), output.value);
        }

        // Return the output oscillator's value
        Audio::new(osc_outputs.get(&self.output_id).copied().unwrap_or(0.0))
    }
}

/// Adapter that implements [`Generator`] for an [`OscillatorGraph`].
struct OscillatorGraphGenerator {
    graph: Arc<Mutex<OscillatorGraph>>,
}

impl Module for OscillatorGraphGenerator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "OscillatorGraphGenerator"
    }
}

impl Generator<Audio> for OscillatorGraphGenerator {
    fn output(&mut self) -> Audio {
        let graph = self.graph.lock().unwrap();
        graph.process_and_output()
    }
}

/// A running oscillator patch with active audio output.
///
/// Call [`stop`](Self::stop) to end playback.
pub struct RunningOscillatorPatch {
    patch: Patch,
    dac: Dac,
}

impl RunningOscillatorPatch {
    /// Stops audio playback and releases the audio device.
    pub fn stop(mut self) {
        self.dac.stop();
    }

    /// Returns a reference to the patch document.
    pub fn patch(&self) -> &Patch {
        &self.patch
    }
}
