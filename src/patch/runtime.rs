//! Patch runtime for executing modular synthesis patches.

use crate::modules::Dac;
use crate::Module;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::graph::RoutingConnection;
use super::graph::SignalGraph;

/// Type alias for module instances stored in the runtime.
pub type ModuleInstance = Arc<Mutex<dyn Module + Send>>;

/// Validates that a module has the specified output port.
pub(crate) fn validate_output_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.lock().unwrap();
    if !m.outputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have output port '{}'. Available: {:?}",
            m.name(),
            port,
            m.outputs()
        )
        .into());
    }
    Ok(())
}

/// Validates that a module has the specified input port.
pub(crate) fn validate_input_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.lock().unwrap();
    if !m.inputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have input port '{}'. Available: {:?}",
            m.name(),
            port,
            m.inputs()
        )
        .into());
    }
    Ok(())
}

/// A prepared patch ready to run.
pub struct PatchRuntime {
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    pub(crate) routing: Vec<RoutingConnection>,
    pub(crate) dac_id: String,
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
                .or_default()
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

        let mut dac = Dac::new()?;

        // Pass a closure that calls process_sample on the graph
        let graph_clone = graph_arc.clone();
        dac.start(move || graph_clone.lock().unwrap().process_sample())?;

        Ok(RunningPatch {
            dac,
            _graph: graph_arc,
        })
    }
}

/// A running patch with audio output.
pub struct RunningPatch {
    dac: Dac,
    _graph: Arc<Mutex<SignalGraph>>,
}

impl RunningPatch {
    /// Stops audio playback.
    pub fn stop(mut self) {
        self.dac.stop();
    }
}
