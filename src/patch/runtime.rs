//! Patch runtime for executing modular synthesis patches.

use crate::modules::{Adsr, Clock, Dac, MelodyGenerator, MelodyParams, Oscillator, Tempo, Vca};
use crate::ModularModule;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::graph::{RoutingConnection, SignalGraph};

/// Runtime instance of a modular module.
pub(crate) enum ModuleInstance {
    Clock(Arc<Mutex<Clock>>),
    Melody(Arc<Mutex<MelodyGenerator>>),
    Oscillator(Arc<Mutex<Oscillator>>),
    Adsr(Arc<Mutex<Adsr>>),
    Vca(Arc<Mutex<Vca>>),
    Dac, // Special case
}

impl ModuleInstance {
    pub(crate) fn as_modular_module(&self) -> Option<Arc<Mutex<dyn ModularModule + Send>>> {
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

    pub(crate) fn validate_output_port(
        &self,
        port: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
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

    pub(crate) fn validate_input_port(&self, port: &str) -> Result<(), Box<dyn std::error::Error>> {
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

/// A prepared patch ready to run.
pub struct PatchRuntime {
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    pub(crate) routing: Vec<RoutingConnection>,
    pub(crate) dac_id: String,
    pub(crate) tempo: Tempo,
    pub(crate) melody_params: Vec<MelodyParams>,
}

impl PatchRuntime {
    /// Starts audio playback.
    pub fn start(self) -> Result<RunningPatch, Box<dyn std::error::Error>> {
        // Build input map: for each module, collect all connections that feed into it
        let mut input_map: std::collections::HashMap<String, Vec<RoutingConnection>> =
            std::collections::HashMap::new();

        for conn in &self.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_insert_with(Vec::new)
                .push(conn.clone());
        }

        let graph = SignalGraph {
            modules: self.modules,
            routing: self.routing,
            dac_id: self.dac_id,
            input_map,
            current_sample: 0,
        };

        let graph_arc = Arc::new(Mutex::new(graph));
        let generator = super::graph::GraphGenerator {
            graph: graph_arc.clone(),
        };

        let mut dac = Dac::new()?;
        dac.start(generator)?;

        Ok(RunningPatch {
            dac,
            tempo: self.tempo,
            melody_params: self.melody_params,
            _graph: graph_arc,
        })
    }
}

/// A running patch with audio output.
pub struct RunningPatch {
    dac: Dac,
    tempo: Tempo,
    melody_params: Vec<MelodyParams>,
    _graph: Arc<Mutex<SignalGraph>>,
}

impl RunningPatch {
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
