//! Module factory traits for self-contained module construction.
//!
//! This module provides the infrastructure for modules to own their build logic.
//! Each module type provides a factory implementation that knows how to construct
//! instances from configuration.

use crate::Module;
use std::any::Any;
use std::sync::{Arc, Mutex};

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
///         })
///     }
/// }
/// ```
pub trait ModuleFactory: Send + Sync + 'static {
    /// Returns the type identifier for this module type.
    ///
    /// This must match the "type" field in JSON patch files.
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
}

/// Result of building a module from a factory.
///
/// Contains both the module instance and any handles for runtime control.
pub struct ModuleBuildResult {
    /// The constructed module instance.
    pub module: Arc<Mutex<dyn Module + Send>>,

    /// Named handles for runtime control.
    ///
    /// Each handle is a `(name, value)` pair where:
    /// - `name` is the handle name (e.g., "tempo", "params")
    /// - `value` is a type-erased handle that users can downcast
    ///
    /// These will be combined with the module ID to form flat keys
    /// like "clock.tempo" or "melody1.params".
    pub handles: Vec<(String, Arc<dyn Any + Send + Sync>)>,
}
