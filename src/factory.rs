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

    /// Constructs a temporary instance for metadata inspection on the control
    /// thread. It must not write files, start workers, or activate outputs.
    /// Factories with such build-time effects must override this method with a
    /// metadata-only implementation or return an explicit error. Asset reads
    /// and ordinary in-memory construction are permitted.
    fn build_for_inspection(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        self.build(sample_rate, config)
    }

    /// Constructs a throwaway instance on the control thread that checks
    /// `config` builds, then is dropped without processing audio. Like
    /// inspection, it must not write files, open streams, or activate
    /// outputs, so checking a document never disturbs the one playing (a
    /// recording at the same path, say). Unlike inspection, it may run what
    /// a live build runs otherwise. The default is
    /// [`Self::build_for_inspection`]; a factory whose inspection refuses
    /// what a live build accepts overrides it.
    fn build_for_validation(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        self.build_for_inspection(sample_rate, config)
    }

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

    /// Returns input ports for type discovery when constructing the module
    /// requires side effects or mandatory config.
    fn input_ports(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// Returns output ports for type discovery when constructing the module
    /// requires side effects or mandatory config.
    fn output_ports(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// Applies the entries of `config` whose keys `selects` picks to `surface`
/// through its setter, as the module's initial values.
///
/// A module's config must accept each of its controls' keys with the
/// control's value: an authored control write records the value into the
/// config under the control's key (see `authored_document::write_control`),
/// and the document must rebuild to the sound that was written. A factory
/// calls this for the control keys its config does not otherwise read,
/// right after making its controls and before making the module, so the
/// module starts from these values; a control key wins over the config key
/// it overlaps, since it records a later write. Unindexed keys (a count) are
/// applied before indexed ones (`degree.3`) so a count sizes what follows.
pub(crate) fn apply_control_keys(
    surface: &dyn crate::ControlSurface,
    config: &serde_json::Value,
    selects: impl Fn(&str) -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(entries) = config.as_object() else {
        return Ok(());
    };
    let mut keys: Vec<&String> = entries.keys().filter(|key| selects(key)).collect();
    keys.sort_by_key(|key| key.contains('.'));
    for key in keys {
        let value = match &entries[key.as_str()] {
            serde_json::Value::Number(number) => {
                crate::ControlValue::Number(number.as_f64().unwrap_or(0.0) as f32)
            }
            serde_json::Value::Bool(flag) => crate::ControlValue::Bool(*flag),
            serde_json::Value::String(text) => crate::ControlValue::String(text.clone()),
            _ => return Err(format!("config '{key}' must be a number, boolean or text").into()),
        };
        let value = surface.coerce_value(key, value);
        surface
            .set_control(key, value)
            .map_err(|error| format!("config '{key}': {error}"))?;
    }
    Ok(())
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

    /// Stereo output blocks for a sink module, valid for `[..frames]` after
    /// [`Module::process`]. Returns `None` for non-sink modules.
    pub fn sink_block(&self) -> Option<(&[f32], &[f32])> {
        match self {
            Self::Module(_) => None,
            Self::Sink(module) => Some(module.sink_block()),
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

/// Inert representation of a sink whose ports are independent of config.
/// Only inspection builds may use it; it never activates an output backend.
pub(crate) fn inspection_sink(
    inputs: &'static [&'static str],
    outputs: &'static [&'static str],
) -> ModuleBuildResult {
    ModuleBuildResult {
        module: GraphModule::Module(Box::new(InspectionSink { inputs, outputs })),
        handles: Vec::new(),
        control_surface: None,
        sink: Some(()),
    }
}

struct InspectionSink {
    inputs: &'static [&'static str],
    outputs: &'static [&'static str],
}

impl Module for InspectionSink {
    fn name(&self) -> &str {
        "inspection sink"
    }
    fn inputs(&self) -> &[&str] {
        self.inputs
    }
    fn outputs(&self) -> &[&str] {
        self.outputs
    }
    fn process(&mut self, _: usize) -> bool {
        panic!("inspection modules cannot process audio")
    }
    fn input_block_mut(&mut self, _: usize) -> &mut [f32] {
        panic!("inspection modules cannot process audio")
    }
    fn output_block(&self, _: usize) -> &[f32] {
        panic!("inspection modules cannot process audio")
    }
    fn set_input(&mut self, _: &str, _: f32) -> Result<(), String> {
        Err("inspection only".into())
    }
    fn get_output(&self, _: &str) -> Result<f32, String> {
        Err("inspection only".into())
    }
}
