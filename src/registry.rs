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
    mode: BuildMode,
}

/// Which of a factory's build methods [`ModuleRegistry::build`] calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildMode {
    /// [`ModuleFactory::build`]: an instance that may run.
    Live,
    /// [`ModuleFactory::build_for_inspection`].
    Inspection,
    /// [`ModuleFactory::build_for_validation`].
    Validation,
}

impl ModuleRegistry {
    /// Creates an empty registry with no factories registered.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            mode: BuildMode::Live,
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
    ///
    /// Config is closed: a key that is neither one of the factory's
    /// [`config_keys`](ModuleFactory::config_keys) nor one of the built
    /// module's controls is refused, naming the type and the key. A sink's
    /// undeclared keys are refused before it is built, since a live sink
    /// build may open a file or a stream.
    pub fn build(
        &self,
        type_id: &str,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let factory = self
            .factories
            .get(type_id)
            .ok_or_else(|| format!("Unknown module type: {}", type_id))?;
        let declared = factory.config_keys();
        let undeclared: Vec<&String> = match config.as_object() {
            Some(entries) if !factory.open_config() => entries
                .keys()
                .filter(|key| declared.iter().all(|d| !declares(d.key, key)))
                .collect(),
            _ => Vec::new(),
        };
        if factory.is_sink() {
            if let Some(key) = undeclared.first() {
                return Err(unknown_config_key(type_id, key, declared, &[]).into());
            }
        }
        let built = match self.mode {
            BuildMode::Live => factory.build(sample_rate, config),
            BuildMode::Inspection => factory.build_for_inspection(sample_rate, config),
            BuildMode::Validation => factory.build_for_validation(sample_rate, config),
        }?;
        if !undeclared.is_empty() {
            let controls = built
                .control_surface
                .as_ref()
                .map(|surface| surface.controls())
                .unwrap_or_default();
            // An indexed key past the family's current count (`degree.6`
            // written before the count shrank) is still the family's.
            let is_control = |key: &str| {
                controls.iter().any(|meta| match meta.key.split_once('.') {
                    Some((stem, _)) => declares(&format!("{stem}.N"), key),
                    None => meta.key == key,
                })
            };
            if let Some(key) = undeclared.into_iter().find(|key| !is_control(key)) {
                return Err(unknown_config_key(type_id, key, declared, &controls).into());
            }
        }
        Ok(built)
    }

    /// Internal registry view that preserves inspection mode through nested
    /// development builds. It must never be used to start an audio runtime.
    pub(crate) fn for_inspection(&self) -> Self {
        let mut registry = self.clone();
        registry.mode = BuildMode::Inspection;
        registry
    }

    /// Internal registry view for throwaway builds that check a document
    /// builds (an edit batch's candidate, say) and are then dropped: every
    /// build goes through [`ModuleFactory::build_for_validation`], so no
    /// output is activated. Preserved through nested development builds. It
    /// must never be used to start an audio runtime.
    pub(crate) fn for_validation(&self) -> Self {
        let mut registry = self.clone();
        registry.mode = BuildMode::Validation;
        registry
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

    /// Returns factory-declared input ports when available.
    pub fn factory_input_ports(&self, type_id: &str) -> Option<&'static [&'static str]> {
        self.factories.get(type_id).and_then(|f| f.input_ports())
    }

    /// Returns factory-declared output ports when available.
    pub fn factory_output_ports(&self, type_id: &str) -> Option<&'static [&'static str]> {
        self.factories.get(type_id).and_then(|f| f.output_ports())
    }

    /// The config keys a type declares (see
    /// [`ModuleFactory::config_keys`]); none for an unknown type.
    pub fn config_keys(&self, type_id: &str) -> &'static [crate::module_config::ConfigKey] {
        self.factories
            .get(type_id)
            .map(|f| f.config_keys())
            .unwrap_or(&[])
    }

    /// Returns an iterator over registered type identifiers.
    pub fn types(&self) -> impl Iterator<Item = &str> + '_ {
        self.factories.keys().map(String::as_str)
    }
}

/// True when declared key `declared` names `key`: exactly, or as an indexed
/// family `stem.N` that takes every `stem.0`, `stem.1`, ….
fn declares(declared: &str, key: &str) -> bool {
    match (declared.strip_suffix(".N"), key.split_once('.')) {
        (Some(stem), Some((key_stem, index))) => {
            stem == key_stem && !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
        }
        _ => declared == key,
    }
}

/// The refusal of config key `key`, listing the keys `type_id` takes, with
/// an indexed family of controls (`level.0`, `level.1`, …) shown once.
fn unknown_config_key(
    type_id: &str,
    key: &str,
    declared: &[crate::module_config::ConfigKey],
    controls: &[crate::ControlMeta],
) -> String {
    let mut keys: Vec<String> = declared.iter().map(|d| d.key.to_string()).collect();
    for meta in controls {
        let key = match meta.key.split_once('.') {
            Some((stem, _)) => format!("{stem}.N"),
            None => meta.key.clone(),
        };
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    format!(
        "{type_id} config has no key '{key}'; it takes {}",
        keys.join(", ")
    )
}

impl Default for ModuleRegistry {
    /// Creates a registry with all built-in module factories.
    fn default() -> Self {
        use crate::modules::{
            AdsrFactory, AgentFactory, AudioFileSinkFactory, CellSequencerFactory, ClockFactory,
            CodeFactory, ControlSchedulerFactory, DacFactory, DivisiFactory, FilterFactory,
            LfoFactory, MelodyFactory, MixerFactory, OscillatorFactory, ReverbFactory,
            SampleInstrumentFactory, SampleKitFactory, SamplePlayerFactory, SampleSlicerFactory,
            StepSequencerFactory, VcaFactory,
        };

        let mut reg = Self::new();
        reg.register(AgentFactory);
        reg.register(AudioFileSinkFactory);
        reg.register(CellSequencerFactory);
        reg.register(ClockFactory);
        reg.register(DivisiFactory);
        reg.register(CodeFactory);
        reg.register(ControlSchedulerFactory);
        reg.register(OscillatorFactory);
        reg.register(LfoFactory);
        reg.register(FilterFactory);
        reg.register(MixerFactory);
        reg.register(AdsrFactory);
        reg.register(VcaFactory);
        reg.register(MelodyFactory);
        reg.register(ReverbFactory);
        #[cfg(not(target_arch = "wasm32"))]
        reg.register(crate::modules::RtmpSinkFactory);
        #[cfg(not(target_arch = "wasm32"))]
        reg.register(crate::modules::YoutubeSinkFactory);
        reg.register(SampleInstrumentFactory);
        reg.register(SampleKitFactory);
        reg.register(SamplePlayerFactory);
        reg.register(SampleSlicerFactory);
        reg.register(StepSequencerFactory);
        reg.register(crate::modules::sustain::SustainFactory);
        reg.register(DacFactory);
        #[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
        reg.register(crate::WasmModuleFactory);
        reg
    }
}
