//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstraction for building synthesis graphs:
//! - [`Module`] - The unified trait for all audio processing components with named ports
//! - [`SinkModule`] - Trait for modules that output to external destinations (audio, file, network)
//! - [`ControlMeta`] - Metadata describing a module control for UI/REPL discovery
use serde::{Deserialize, Serialize};

/// Runtime value for a module control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Number(f32),
    Bool(bool),
    String(String),
}

impl ControlValue {
    pub fn as_number(&self) -> Result<f32, String> {
        match self {
            Self::Number(value) => Ok(*value),
            _ => Err("Expected numeric control value".to_string()),
        }
    }

    pub fn as_bool(&self) -> Result<bool, String> {
        match self {
            Self::Bool(value) => Ok(*value),
            _ => Err("Expected boolean control value".to_string()),
        }
    }

    pub fn as_string(&self) -> Result<&str, String> {
        match self {
            Self::String(value) => Ok(value),
            _ => Err("Expected string control value".to_string()),
        }
    }
}

impl From<f32> for ControlValue {
    fn from(value: f32) -> Self {
        Self::Number(value)
    }
}

impl From<bool> for ControlValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<String> for ControlValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for ControlValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

/// Type-specific metadata describing a control value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlKind {
    Number { min: f32, max: f32 },
    Bool,
    String { options: Option<Vec<String>> },
}

/// Metadata about a single control exposed by a module.
///
/// Controls are parameters that can be adjusted at runtime via user interaction
/// (knobs, sliders, buttons). This metadata enables UIs to display appropriate
/// widgets with correct ranges and labels.
///
/// # Example
///
/// ```rust,ignore
/// ControlMeta {
///     key: "attack".to_string(),
///     description: "Attack time in seconds".to_string(),
///     default: ControlValue::Number(0.01),
///     kind: ControlKind::Number { min: 0.0, max: 10.0 },
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlMeta {
    /// The control key (e.g., "attack", "level.0", "type")
    pub key: String,
    /// Human-readable description
    pub description: String,
    /// Default value
    pub default: ControlValue,
    /// Value constraints and editor hints
    pub kind: ControlKind,
}

impl ControlMeta {
    /// Legacy alias for creating a numeric control metadata entry.
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self::number(key, description)
    }

    /// Creates a numeric control metadata entry.
    pub fn number(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::Number(0.0),
            kind: ControlKind::Number { min: 0.0, max: 1.0 },
        }
    }

    /// Sets the range (min, max) for this control.
    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        self.kind = ControlKind::Number { min, max };
        self
    }

    /// Sets the default value for this control.
    pub fn with_default(mut self, default: impl Into<ControlValue>) -> Self {
        self.default = default.into();
        self
    }

    /// Creates a boolean control metadata entry.
    pub fn boolean(key: impl Into<String>, description: impl Into<String>, default: bool) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::Bool(default),
            kind: ControlKind::Bool,
        }
    }

    /// Creates a string control metadata entry.
    pub fn string(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::String(String::new()),
            kind: ControlKind::String { options: None },
        }
    }

    /// Sets the allowed values for a string control.
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        let default_option = options.first().cloned().unwrap_or_else(String::new);
        self.kind = ControlKind::String {
            options: Some(options),
        };
        if !matches!(self.default, ControlValue::String(_)) {
            self.default = ControlValue::String(default_option);
        }
        self
    }

    /// Legacy alias for enumerated controls.
    pub fn with_variants(self, variants: Vec<String>) -> Self {
        self.with_options(variants)
    }
}

/// Shared runtime control surface for a module.
pub trait ControlSurface: Send + Sync {
    fn controls(&self) -> Vec<ControlMeta>;
    fn get_control(&self, key: &str) -> Result<ControlValue, String>;
    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String>;
}

/// The core abstraction for all synthesis components.
///
/// Every module in the synthesis graph implements this trait.
/// Modules process one sample at a time at audio rate, with named input and output ports.
///
/// All signals are `f32` values. The meaning of a signal is determined by which port
/// it connects to, not by its type. This design mirrors real modular synthesizers where
/// everything is voltage.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::Module;
///
/// struct Vca {
///     audio_in: f32,
///     cv_in: f32,
///     last_processed_sample: u64,
/// }
///
/// impl Module for Vca {
///     fn name(&self) -> &str {
///         "Vca"
///     }
///
///     fn process(&mut self) -> bool {
///         // Processing happens here
///         true
///     }
///
///     fn inputs(&self) -> &[&str] {
///         &["audio", "cv"]
///     }
///
///     fn outputs(&self) -> &[&str] {
///         &["audio"]
///     }
///
///     fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
///         match port {
///             "audio" => { self.audio_in = value; Ok(()) }
///             "cv" => { self.cv_in = value; Ok(()) }
///             _ => Err(format!("Unknown input port: {}", port))
///         }
///     }
///
///     fn get_output(&self, port: &str) -> Result<f32, String> {
///         match port {
///             "audio" => Ok(self.audio_in * self.cv_in),
///             _ => Err(format!("Unknown output port: {}", port))
///         }
///     }
///
///     fn last_processed_sample(&self) -> u64 {
///         self.last_processed_sample
///     }
///
///     fn mark_processed(&mut self, sample: u64) {
///         self.last_processed_sample = sample;
///     }
/// }
/// ```
pub trait Module: Send {
    /// Returns the module's name for debugging purposes.
    fn name(&self) -> &str;

    /// Processes one sample of audio.
    ///
    /// Returns `true` if the module is still active, `false` if it should be removed.
    fn process(&mut self) -> bool;

    /// Returns the names of all input ports this module accepts.
    ///
    /// Port names should be stable and descriptive (e.g., "frequency", "gate", "fm", "cv").
    fn inputs(&self) -> &[&str];

    /// Returns the names of all output ports this module provides.
    ///
    /// Port names should be stable and descriptive (e.g., "audio", "trigger", "envelope").
    fn outputs(&self) -> &[&str];

    /// Sets the value for a named input port.
    ///
    /// Called once per sample by the invention runtime for each connected input.
    /// Returns an error if the port name is not recognized.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the input port (must match one from `inputs()`)
    /// * `value` - The signal value (typically -1.0 to 1.0 for audio, 0.0 to 1.0 for CV)
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String>;

    /// Gets the current value from a named output port.
    ///
    /// Returns an error if the port name is not recognized.
    /// This method should not modify module state - it only reads cached values.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the output port (must match one from `outputs()`)
    fn get_output(&self, port: &str) -> Result<f32, String>;

    /// Returns the sample number when this module was last processed.
    ///
    /// Used by the pull-based processing system to avoid reprocessing modules
    /// multiple times within a single sample period. Modules should initialize
    /// this to 0 and update it via `mark_processed()` after each processing cycle.
    fn last_processed_sample(&self) -> u64;

    /// Marks this module as processed for the given sample number.
    ///
    /// Called by the signal graph after processing the module. This enables
    /// caching: if the same module's output is requested multiple times in
    /// one sample, it returns cached values without reprocessing.
    fn mark_processed(&mut self, sample: u64);

    /// Resets input "active" flags before each sample.
    ///
    /// Called by the signal graph before routing signals for each sample.
    /// Modules use this to track which inputs received signals vs. using
    /// control defaults.
    ///
    /// Default implementation does nothing.
    fn reset_inputs(&mut self) {}

    /// Legacy module-local control metadata surface.
    fn controls(&self) -> Vec<ControlMeta> {
        vec![]
    }

    /// Legacy module-local numeric control getter.
    fn get_control(&self, _key: &str) -> Result<f32, String> {
        Err("Module has no controls".to_string())
    }

    /// Legacy module-local numeric control setter.
    fn set_control(&mut self, _key: &str, _value: f32) -> Result<(), String> {
        Err("Module has no controls".to_string())
    }
}

/// Output from a sink module.
///
/// Supports stereo output. Mono sources should use [`SinkOutput::mono`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SinkOutput {
    pub left: f32,
    pub right: f32,
}

impl SinkOutput {
    /// Creates a mono sink output.
    pub fn mono(sample: f32) -> Self {
        Self {
            left: sample,
            right: sample,
        }
    }

    /// Creates a stereo sink output.
    pub fn stereo(left: f32, right: f32) -> Self {
        Self { left, right }
    }
}

/// A module that collects output for external destinations.
///
/// Sink modules are the final stage in signal chains, sending audio to
/// destinations like audio devices (DAC), files, or network streams.
/// They drive the pull-based processing: the signal graph pulls from
/// all sink modules each sample, which triggers recursive processing
/// of their upstream dependencies.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::{Module, SinkModule, SinkOutput};
///
/// struct DacModule {
///     audio_in: f32,
///     last_processed_sample: u64,
/// }
///
/// impl SinkModule for DacModule {
///     fn sink_output(&self) -> SinkOutput {
///         SinkOutput::mono(self.audio_in)
///     }
/// }
/// ```
pub trait SinkModule: Module {
    /// Returns the collected output after processing.
    ///
    /// Called by the signal graph after `process()` to collect the
    /// final output sample for mixing to audio output.
    fn sink_output(&self) -> SinkOutput;
}

/// Helper for validating port names at module construction.
///
/// Returns `Ok(())` if the port name is in the list, `Err` otherwise.
pub fn validate_port(port: &str, valid_ports: &[&str], port_type: &str) -> Result<(), String> {
    if valid_ports.contains(&port) {
        Ok(())
    } else {
        Err(format!(
            "Unknown {} port '{}'. Valid ports: {:?}",
            port_type, port, valid_ports
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestModule {
        input_a: f32,
        input_b: f32,
        last_processed_sample: u64,
    }

    impl Module for TestModule {
        fn name(&self) -> &str {
            "TestModule"
        }

        fn process(&mut self) -> bool {
            true
        }

        fn inputs(&self) -> &[&str] {
            &["a", "b"]
        }

        fn outputs(&self) -> &[&str] {
            &["sum", "product"]
        }

        fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
            match port {
                "a" => {
                    self.input_a = value;
                    Ok(())
                }
                "b" => {
                    self.input_b = value;
                    Ok(())
                }
                _ => Err(format!("Unknown input port: {}", port)),
            }
        }

        fn get_output(&self, port: &str) -> Result<f32, String> {
            match port {
                "sum" => Ok(self.input_a + self.input_b),
                "product" => Ok(self.input_a * self.input_b),
                _ => Err(format!("Unknown output port: {}", port)),
            }
        }

        fn last_processed_sample(&self) -> u64 {
            self.last_processed_sample
        }

        fn mark_processed(&mut self, sample: u64) {
            self.last_processed_sample = sample;
        }
    }

    #[test]
    fn test_module() {
        let mut module = TestModule {
            input_a: 0.0,
            input_b: 0.0,
            last_processed_sample: 0,
        };

        // Test setting inputs
        assert!(module.set_input("a", 3.0).is_ok());
        assert!(module.set_input("b", 4.0).is_ok());
        assert!(module.set_input("c", 5.0).is_err());

        // Test getting outputs
        assert_eq!(module.get_output("sum").unwrap(), 7.0);
        assert_eq!(module.get_output("product").unwrap(), 12.0);
        assert!(module.get_output("invalid").is_err());

        // Test sample tracking
        assert_eq!(module.last_processed_sample(), 0);
        module.mark_processed(42);
        assert_eq!(module.last_processed_sample(), 42);

        // Test output after input change
        module.set_input("a", 5.0).unwrap();
        module.set_input("b", 6.0).unwrap();
        assert_eq!(module.get_output("sum").unwrap(), 11.0);
        assert_eq!(module.get_output("product").unwrap(), 30.0);
        assert!(module.get_output("invalid").is_err());
    }

    #[test]
    fn test_validate_port() {
        let ports = &["audio", "cv", "gate"];

        assert!(validate_port("audio", ports, "input").is_ok());
        assert!(validate_port("cv", ports, "input").is_ok());
        assert!(validate_port("invalid", ports, "input").is_err());
    }
}
