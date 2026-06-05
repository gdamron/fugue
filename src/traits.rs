//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstraction for building synthesis graphs:
//! - [`Module`] - The unified trait for all audio processing components with named ports
//! - [`SinkModule`] - Trait for modules that output to external destinations (audio, file, network)
//! - [`ControlMeta`] - Metadata describing a module control for UI/REPL discovery
use serde::{Deserialize, Serialize};

/// Maximum number of frames the engine processes in a single block.
///
/// Modules size their per-port buffers to this length; the signal graph never
/// requests a `frames` count larger than `MAX_BLOCK` in a single
/// [`Module::process`] call. The actual block size used at runtime is
/// configurable (see [`crate::RenderEngine`]) and defaults to
/// [`DEFAULT_BLOCK_SIZE`], but it is always `<= MAX_BLOCK`.
pub const MAX_BLOCK: usize = 1024;

/// Default audio processing block size in frames.
///
/// Chosen as a DAW-typical balance between per-call amortization and
/// feedback/control latency. Configurable per engine instance.
pub const DEFAULT_BLOCK_SIZE: usize = 64;

/// Runtime value for a module control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
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
/// Modules process a **block** of audio frames at a time. Each module owns one
/// pre-allocated buffer per input and output port (sized to [`MAX_BLOCK`]). The
/// signal graph copies upstream output blocks into a module's input buffers,
/// calls [`Module::process`] for the block, then reads the module's output
/// buffers to feed downstream modules.
///
/// All signals are `f32` values. The meaning of a signal is determined by which port
/// it connects to, not by its type. This design mirrors real modular synthesizers where
/// everything is voltage.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::{Module, MAX_BLOCK};
///
/// struct Vca {
///     audio_in: [f32; MAX_BLOCK],
///     cv_in: [f32; MAX_BLOCK],
///     audio_out: [f32; MAX_BLOCK],
/// }
///
/// impl Module for Vca {
///     fn name(&self) -> &str { "Vca" }
///
///     fn inputs(&self) -> &[&str] { &["audio", "cv"] }
///     fn outputs(&self) -> &[&str] { &["audio"] }
///
///     fn process(&mut self, frames: usize) -> bool {
///         for i in 0..frames {
///             self.audio_out[i] = self.audio_in[i] * self.cv_in[i];
///         }
///         true
///     }
///
///     fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
///         match index {
///             0 => &mut self.audio_in,
///             _ => &mut self.cv_in,
///         }
///     }
///
///     fn output_block(&self, _index: usize) -> &[f32] { &self.audio_out }
///
///     fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
///         let buf = match port {
///             "audio" => &mut self.audio_in,
///             "cv" => &mut self.cv_in,
///             _ => return Err(format!("Unknown input port: {}", port)),
///         };
///         buf.fill(value);
///         Ok(())
///     }
///
///     fn get_output(&self, port: &str) -> Result<f32, String> {
///         match port {
///             "audio" => Ok(self.audio_out[0]),
///             _ => Err(format!("Unknown output port: {}", port)),
///         }
///     }
/// }
/// ```
pub trait Module: Send {
    /// Returns the module's name for debugging purposes.
    fn name(&self) -> &str;

    /// Processes a block of `frames` audio frames (always `<= MAX_BLOCK`).
    ///
    /// On entry, each connected input port's buffer holds `frames` samples of
    /// upstream signal (see [`Module::input_block_mut`]); unconnected input
    /// buffers hold silence (zeros) unless the module arbitrates via
    /// [`Module::set_input_connected`]. The module must write `frames` samples
    /// to each of its output port buffers.
    ///
    /// Returns `true` if the module is still active, `false` if it should be removed.
    fn process(&mut self, frames: usize) -> bool;

    /// Returns the names of all input ports this module accepts.
    ///
    /// Port names should be stable and descriptive (e.g., "frequency", "gate", "fm", "cv").
    fn inputs(&self) -> &[&str];

    /// Returns the names of all output ports this module provides.
    ///
    /// Port names should be stable and descriptive (e.g., "audio", "trigger", "envelope").
    fn outputs(&self) -> &[&str];

    /// Mutable access to an input port's block buffer.
    ///
    /// The signal graph copies the upstream output block into `[..frames]`
    /// before calling [`Module::process`]. The returned slice must be at least
    /// `MAX_BLOCK` long. Called on the audio hot path; must not allocate.
    fn input_block_mut(&mut self, index: usize) -> &mut [f32];

    /// Read-only access to an output port's block buffer.
    ///
    /// Valid for `[..frames]` after [`Module::process`] has run. The returned
    /// slice must be at least `MAX_BLOCK` long. Called on the audio hot path;
    /// must not allocate.
    fn output_block(&self, index: usize) -> &[f32];

    /// Sets a named input port to a constant value across its whole buffer.
    ///
    /// Convenience for the control thread (the `SetModuleInput` command) and
    /// tests — not the per-block routing path. Modules that arbitrate between a
    /// connected signal and a control default should also mark the port
    /// connected here. Returns an error if the port name is not recognized.
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String>;

    /// Gets the most recent value from a named output port (frame 0 of the
    /// output block). Convenience for tests/inspection, not the hot path.
    ///
    /// Returns an error if the port name is not recognized.
    fn get_output(&self, port: &str) -> Result<f32, String>;

    /// Resolves an input port name to a stable index. Topology-change path,
    /// not the audio hot path.
    fn input_port_index(&self, name: &str) -> Option<usize> {
        self.inputs().iter().position(|n| *n == name)
    }

    /// Resolves an output port name to a stable index. Topology-change path.
    fn output_port_index(&self, name: &str) -> Option<usize> {
        self.outputs().iter().position(|n| *n == name)
    }

    /// Declares whether an input port is fed by an upstream connection.
    ///
    /// Called by the signal graph on topology change (never on the hot path),
    /// once per input port. Modules that arbitrate between an incoming signal
    /// and a control default (e.g. an oscillator's `frequency` port) override
    /// this to record connectivity. The default ignores it — most modules
    /// simply read their input buffers (which are silence when unconnected).
    fn set_input_connected(&mut self, _index: usize, _connected: bool) {}

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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
/// use fugue::{Module, SinkModule, MAX_BLOCK};
///
/// struct DacModule {
///     left: [f32; MAX_BLOCK],
///     right: [f32; MAX_BLOCK],
/// }
///
/// impl SinkModule for DacModule {
///     fn sink_block(&self) -> (&[f32], &[f32]) {
///         (&self.left, &self.right)
///     }
/// }
/// ```
pub trait SinkModule: Module {
    /// Returns the collected stereo output blocks after [`Module::process`].
    ///
    /// Both slices are valid for `[..frames]` after processing the block. The
    /// signal graph mixes these into the engine's interleaved output. Mono
    /// sinks return the same buffer for both channels.
    fn sink_block(&self) -> (&[f32], &[f32]);
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
        input_a: [f32; MAX_BLOCK],
        input_b: [f32; MAX_BLOCK],
        sum: [f32; MAX_BLOCK],
        product: [f32; MAX_BLOCK],
    }

    impl TestModule {
        fn new() -> Self {
            Self {
                input_a: [0.0; MAX_BLOCK],
                input_b: [0.0; MAX_BLOCK],
                sum: [0.0; MAX_BLOCK],
                product: [0.0; MAX_BLOCK],
            }
        }
    }

    impl Module for TestModule {
        fn name(&self) -> &str {
            "TestModule"
        }

        fn process(&mut self, frames: usize) -> bool {
            for i in 0..frames {
                self.sum[i] = self.input_a[i] + self.input_b[i];
                self.product[i] = self.input_a[i] * self.input_b[i];
            }
            true
        }

        fn inputs(&self) -> &[&str] {
            &["a", "b"]
        }

        fn outputs(&self) -> &[&str] {
            &["sum", "product"]
        }

        fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
            match index {
                0 => &mut self.input_a,
                _ => &mut self.input_b,
            }
        }

        fn output_block(&self, index: usize) -> &[f32] {
            match index {
                0 => &self.sum,
                _ => &self.product,
            }
        }

        fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
            match port {
                "a" => {
                    self.input_a.fill(value);
                    Ok(())
                }
                "b" => {
                    self.input_b.fill(value);
                    Ok(())
                }
                _ => Err(format!("Unknown input port: {}", port)),
            }
        }

        fn get_output(&self, port: &str) -> Result<f32, String> {
            match port {
                "sum" => Ok(self.sum[0]),
                "product" => Ok(self.product[0]),
                _ => Err(format!("Unknown output port: {}", port)),
            }
        }
    }

    #[test]
    fn test_module() {
        let mut module = TestModule::new();

        // Test setting inputs
        assert!(module.set_input("a", 3.0).is_ok());
        assert!(module.set_input("b", 4.0).is_ok());
        assert!(module.set_input("c", 5.0).is_err());

        // Process a one-frame block and read outputs.
        module.process(1);
        assert_eq!(module.get_output("sum").unwrap(), 7.0);
        assert_eq!(module.get_output("product").unwrap(), 12.0);
        assert!(module.get_output("invalid").is_err());

        // Test output after input change
        module.set_input("a", 5.0).unwrap();
        module.set_input("b", 6.0).unwrap();
        module.process(1);
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
