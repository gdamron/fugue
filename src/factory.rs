//! Module factory traits for self-contained module construction.
//!
//! This module provides the infrastructure for modules to own their build logic.
//! Each module type provides a factory implementation that knows how to construct
//! instances from configuration.

use crate::{ControlSurface, Module, SinkModule};
use std::any::Any;
use std::sync::Arc;

/// Factory for constructing module instances from configuration.
///
/// Each module type provides its own factory implementation. Factories are
/// registered with a [`ModuleRegistry`](crate::ModuleRegistry) for lookup by type name.
///
/// # Example
///
/// ```rust,ignore
/// pub struct MyModuleFactory;
///
/// impl ModuleFactory for MyModuleFactory {
///     fn type_id(&self) -> &'static str { "my_module" }
///
///     fn build(
///         &self,
///         sample_rate: u32,
///         config: &serde_json::Value,
///     ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
///         let module = MyModule::new(sample_rate);
///         Ok(ModuleBuildResult {
///             module: Arc::new(Mutex::new(module)),
///             handles: vec![],
///             sink: None,
///         })
///     }
/// }
/// ```
pub trait ModuleFactory: Send + Sync + 'static {
    /// Returns the type identifier for this module type.
    ///
    /// This must match the "type" field in invention JSON files.
    /// Examples: "clock", "oscillator", "adsr", "vca", "melody"
    fn type_id(&self) -> &'static str;

    /// Builds a module instance from configuration.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The audio sample rate in Hz
    /// * `config` - Module-specific configuration as JSON
    ///
    /// # Returns
    ///
    /// A `ModuleBuildResult` containing the module instance and any runtime handles.
    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>>;

    /// Returns true if this factory produces sink modules.
    ///
    /// Sink modules are final destinations in the signal chain (e.g., audio output,
    /// file writer, network streamer). They drive pull-based processing and their
    /// outputs are collected for external destinations.
    ///
    /// Default is `false`. Override to return `true` for sink module factories.
    fn is_sink(&self) -> bool {
        false
    }
}

/// Owned module storage used by the signal graph.
pub enum GraphModule {
    Module(Box<dyn Module + Send>),
    Sink(Box<dyn SinkModule + Send>),
}

impl GraphModule {
    pub fn module(&self) -> &dyn Module {
        match self {
            Self::Module(module) => module.as_ref(),
            Self::Sink(module) => module.as_ref(),
        }
    }

    pub fn module_mut(&mut self) -> &mut dyn Module {
        match self {
            Self::Module(module) => module.as_mut(),
            Self::Sink(module) => module.as_mut(),
        }
    }

    pub fn sink_output(&self) -> Option<crate::SinkOutput> {
        match self {
            Self::Module(_) => None,
            Self::Sink(module) => Some(module.sink_output()),
        }
    }
}

/// Result of building a module from a factory.
///
/// Contains both the module instance and any handles for runtime control.
pub struct ModuleBuildResult {
    /// The constructed module instance.
    pub module: GraphModule,

    /// Named handles for runtime control.
    ///
    /// Each handle is a `(name, value)` pair where:
    /// - `name` is the handle name (e.g., "tempo", "params")
    /// - `value` is a type-erased handle that users can downcast
    ///
    /// These will be combined with the module ID to form flat keys
    /// like "clock.tempo" or "melody1.params".
    pub handles: Vec<(String, Arc<dyn Any + Send + Sync>)>,

    /// Shared typed control surface for runtime control, if the module exposes one.
    pub control_surface: Option<Arc<dyn ControlSurface + Send + Sync>>,

    /// Whether this module is a sink.
    ///
    /// Sink modules are represented by [`GraphModule::Sink`] so the signal graph
    /// can process and collect output from the same owned object without locks.
    pub sink: Option<()>,
}
