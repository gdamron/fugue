//! Module registry for factory lookup by type name.
//!
//! The registry provides a central place for mapping module type names
//! (like "clock", "oscillator") to their factory implementations.

use crate::factory::{ModuleBuildResult, ModuleFactory};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of module factories for lookup by type name.
///
/// The registry maps type identifiers (strings like "clock", "oscillator")
/// to factory implementations that can construct modules from configuration.
///
/// # Default Registry
///
/// The default registry includes all built-in module types:
/// - `clock` - Timing and tempo control
/// - `oscillator` - Waveform generation
/// - `adsr` - Envelope generator
/// - `vca` - Voltage controlled amplifier
/// - `melody` - Algorithmic melody generation
///
/// # Example
///
/// ```rust,ignore
/// // Use the default registry
/// let registry = ModuleRegistry::default();
///
/// // Or create a custom registry
/// let mut registry = ModuleRegistry::new();
/// registry.register(ClockFactory);
/// registry.register(OscillatorFactory);
/// ```
#[derive(Clone)]
pub struct ModuleRegistry {
    factories: HashMap<String, Arc<dyn ModuleFactory>>,
}

impl ModuleRegistry {
    /// Creates an empty registry with no factories registered.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Registers a module factory.
    ///
    /// The factory's `type_id()` is used as the key for lookup.
    /// If a factory with the same type_id already exists, it will be replaced.
    pub fn register<F: ModuleFactory>(&mut self, factory: F) {
        self.factories
            .insert(factory.type_id().to_string(), Arc::new(factory));
    }

    /// Registers a boxed factory with a runtime-provided type id.
    pub fn register_boxed(&mut self, type_id: impl Into<String>, factory: Arc<dyn ModuleFactory>) {
        self.factories.insert(type_id.into(), factory);
    }

    /// Builds a module by type name.
    ///
    /// # Arguments
    ///
    /// * `type_id` - The module type (e.g., "clock", "oscillator")
    /// * `sample_rate` - The audio sample rate in Hz
    /// * `config` - Module-specific configuration as JSON
    ///
    /// # Errors
    ///
    /// Returns an error if the type_id is not registered or if the factory
    /// fails to build the module.
    pub fn build(
        &self,
        type_id: &str,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        self.factories
            .get(type_id)
            .ok_or_else(|| format!("Unknown module type: {}", type_id))?
            .build(sample_rate, config)
    }

    /// Returns true if a factory is registered for the given type.
    pub fn has_type(&self, type_id: &str) -> bool {
        self.factories.contains_key(type_id)
    }

    /// Returns true if the given module type is a sink.
    ///
    /// Sink modules are final destinations that drive pull-based processing
    /// and collect output for external destinations (audio devices, files, etc.).
    pub fn is_sink(&self, type_id: &str) -> bool {
        self.factories
            .get(type_id)
            .map(|f| f.is_sink())
            .unwrap_or(false)
    }

    /// Returns an iterator over registered type identifiers.
    pub fn types(&self) -> impl Iterator<Item = &str> + '_ {
        self.factories.keys().map(String::as_str)
    }
}

impl Default for ModuleRegistry {
    /// Creates a registry with all built-in module factories.
    fn default() -> Self {
        use crate::modules::{
            AdsrFactory, AgentFactory, CellSequencerFactory, ClockFactory, CodeFactory,
            DacFactory, FilterFactory, LfoFactory, MelodyFactory, MixerFactory,
            OscillatorFactory, ReverbFactory, SamplePlayerFactory, StepSequencerFactory,
            VcaFactory,
        };

        let mut reg = Self::new();
        reg.register(AgentFactory);
        reg.register(CellSequencerFactory);
        reg.register(ClockFactory);
        reg.register(CodeFactory);
        reg.register(OscillatorFactory);
        reg.register(LfoFactory);
        reg.register(FilterFactory);
        reg.register(MixerFactory);
        reg.register(AdsrFactory);
        reg.register(VcaFactory);
        reg.register(MelodyFactory);
        reg.register(ReverbFactory);
        reg.register(SamplePlayerFactory);
        reg.register(StepSequencerFactory);
        reg.register(DacFactory);
        reg
    }
}
