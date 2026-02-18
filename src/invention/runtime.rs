//! Invention runtime for executing modular synthesis inventions.

use crate::modules::{AudioBackend, AudioDriver};
use crate::Module;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::graph::{RoutingConnection, SignalGraph, SinkInstance};

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

/// A prepared invention ready to run.
pub struct InventionRuntime {
    /// All modules in the invention.
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Sink modules that drive processing.
    pub(crate) sinks: IndexMap<String, SinkInstance>,
    /// Signal routing connections.
    pub(crate) routing: Vec<RoutingConnection>,
}

impl InventionRuntime {
    /// Starts audio playback using the default AudioDriver.
    pub fn start(self) -> Result<RunningInvention, Box<dyn std::error::Error>> {
        let audio = AudioDriver::new()?;
        self.start_with_backend(audio)
    }

    /// Starts audio playback with a custom audio backend.
    ///
    /// This allows using alternative audio backends such as file writers,
    /// network streamers, or null outputs for testing.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Use a custom audio backend
    /// let backend = MyCustomBackend::new();
    /// let running = runtime.start_with_backend(backend)?;
    /// ```
    pub fn start_with_backend<B: AudioBackend + 'static>(
        self,
        mut backend: B,
    ) -> Result<RunningInvention, Box<dyn std::error::Error>> {
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
            sinks: self.sinks,
            input_map,
            current_sample: 0,
        };

        let graph_arc = Arc::new(Mutex::new(graph));

        // Pass a closure that calls process_sample on the graph
        let graph_clone = graph_arc.clone();
        backend.start(Box::new(move || {
            graph_clone.lock().unwrap().process_sample()
        }))?;

        Ok(RunningInvention {
            backend: Box::new(backend),
            _graph: graph_arc,
        })
    }
}

/// A running invention with audio output.
pub struct RunningInvention {
    backend: Box<dyn AudioBackend>,
    _graph: Arc<Mutex<SignalGraph>>,
}

impl RunningInvention {
    /// Stops audio playback.
    pub fn stop(mut self) {
        self.backend.stop();
    }
}
