//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstraction for building synthesis graphs:
//! - [`Module`] - The unified trait for all audio processing components with named ports
//! - [`SinkModule`] - Trait for modules that output to external destinations (audio, file, network)
//! - [`ControlMeta`] - Metadata describing a module control for UI/REPL discovery
use serde::{Deserialize, Serialize};

use crate::control_request::{
    Automation, ControlCells, ControlIndex, ControlTable, Refusal, RtValue,
};
use crate::invention::declared::{Declaration, Route};

mod control_meta;
mod control_validation;

pub use control_validation::ControlSurfaceMap;
pub(crate) use control_validation::{check_finite, check_listed_control, read_only};

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

    /// Best-effort coercion of this value to the shape declared by `kind`.
    ///
    /// MCP clients (and other JSON front-ends) sometimes deliver every control
    /// value as a string — a numeric control receives `"0.74"` rather than
    /// `0.74` — or hand a bare number to a string control. Coercing against the
    /// control's own declared kind lets those writes land without guessing:
    /// the kind states exactly what the control expects. Values that already
    /// match the kind, or that cannot represent it (e.g. a non-numeric string
    /// for a number control), pass through unchanged so the module setter stays
    /// the single source of validation errors.
    pub fn coerced_to(self, kind: &ControlKind) -> ControlValue {
        match kind {
            ControlKind::Number { .. } => match self {
                Self::String(value) => match value.trim().parse::<f32>() {
                    Ok(number) => Self::Number(number),
                    Err(_) => Self::String(value),
                },
                other => other,
            },
            ControlKind::Bool => match self {
                Self::String(value) => match value.trim() {
                    "true" => Self::Bool(true),
                    "false" => Self::Bool(false),
                    _ => Self::String(value),
                },
                other => other,
            },
            ControlKind::String { .. } => match self {
                Self::Number(value) => Self::String(value.to_string()),
                Self::Bool(value) => Self::String(value.to_string()),
                other => other,
            },
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

/// Shared runtime control surface for a module.
pub trait ControlSurface: Send + Sync {
    fn controls(&self) -> Vec<ControlMeta>;
    fn get_control(&self, key: &str) -> Result<ControlValue, String>;
    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String>;

    /// Ids of modules whose controls this module writes while processing,
    /// mirroring [`Module::control_targets`] for the module behind this
    /// surface. A live graph compiles its process order on the control
    /// thread, where only the surface is reachable, so a module that declares
    /// control targets must report the same targets here.
    ///
    /// Control-thread only; implementations may lock and allocate.
    fn control_targets(&self) -> Vec<String> {
        Vec::new()
    }

    /// Checks, changing nothing, that [`Self::set_control`] would accept
    /// `value` (already coerced, see [`Self::coerce_value`]) for `key`, so a
    /// batch of writes can be refused before any of them lands. `surfaces`
    /// is the directory as it will be when the write lands, for values that
    /// name other modules.
    ///
    /// Refuses what the setter refuses before it changes anything: unknown
    /// and read-only keys, values of the wrong kind, numbers that are not
    /// finite, and strings the setter cannot parse (unknown options,
    /// malformed JSON). Only what a write alone can discover, such as a
    /// sample failing to load, may still fail when set. The default checks
    /// the kind `key` declares in [`Self::controls`]; surfaces with
    /// read-only or parsed controls override it.
    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        let _ = surfaces;
        check_listed_control(&self.controls(), key, value)
    }

    /// Binds a declared module's surface to the `route` its writes take
    /// from now on, applying to `module` (the instance behind this surface,
    /// not yet running) the controls written while it was building.
    /// Called once by whatever is about to run the module; legacy surfaces
    /// ignore it.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn bind(&self, route: Route, module: &mut dyn Module) {
        let _ = (route, module);
    }

    /// Opens a surface [`bound`](Self::bind) to a change still being
    /// prepared to `route`, once that change has committed. Legacy surfaces
    /// ignore it.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn activate(&self, route: Route) {
        let _ = route;
    }

    /// Refuses every later write: the module behind this surface was
    /// removed or replaced. Legacy surfaces ignore it.
    #[doc(hidden)]
    fn retire(&self) {}

    /// Whether `key` is one of this surface's declared controls. Such a
    /// control is never written through [`Self::set_control`] from the
    /// audio thread: automation writes it, or it cannot be scheduled.
    #[doc(hidden)]
    fn declares(&self, key: &str) -> bool {
        let _ = key;
        false
    }

    /// The declared control `key`, for a development aliasing it, or `None`
    /// for a legacy surface. Control thread.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn declaration(&self, key: &str) -> Option<Declaration> {
        let _ = key;
        None
    }

    /// The declared control `key`, as automation on the audio thread writes
    /// it (see [`crate::control_request::Automation`]), or `None` for a
    /// legacy surface, or a control automation cannot write. Control
    /// thread: resolved once, when a schedule is.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn automation(&self, key: &str) -> Option<Automation> {
        let _ = key;
        None
    }

    /// Coerces `value` to `key`'s declared [`ControlKind`] via
    /// [`ControlValue::coerced_to`]. Unknown keys pass through untouched so
    /// [`ControlSurface::set_control`] still owns the "unknown control" error.
    /// Control-plane callers use this so a write survives a client that
    /// stringifies values; it is not for the audio thread (it allocates via
    /// [`ControlSurface::controls`]).
    fn coerce_value(&self, key: &str, value: ControlValue) -> ControlValue {
        match self.controls().into_iter().find(|meta| meta.key == key) {
            Some(meta) => value.coerced_to(&meta.kind),
            None => value,
        }
    }
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
    /// upstream signal (see [`Module::input_block_mut`]); an unconnected
    /// input buffer holds silence (zeros), or the value last written to it
    /// with [`Module::set_input`], which graph edits leave in place while the
    /// port stays unconnected. Modules may arbitrate via
    /// [`Module::set_input_connected`]. The module must write `frames` samples
    /// to each of its output port buffers.
    ///
    /// Returns `true` if the module is still active, `false` if it should be removed.
    fn process(&mut self, frames: usize) -> bool;

    /// Returns the names of all input ports this module accepts.
    ///
    /// Port names should be stable and descriptive (e.g., "frequency", "gate", "frequency_mod", "level").
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
    /// Convenience for queued input writes (applied at the start of a block)
    /// and tests — not the per-block routing path. Modules that arbitrate between a
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
    /// Called by the signal graph on topology change (never on the hot path):
    /// with `true` for every connected port, and with `false` for a port it
    /// disconnects or, on a module starting fresh, finds unconnected. A port
    /// that stays unconnected is not declared again, so whatever
    /// [`Module::set_input`] recorded for it stands. Modules that arbitrate
    /// between an incoming signal and a control default (e.g. an
    /// oscillator's `frequency` port) override this to record connectivity.
    /// The default ignores it — most modules simply read their input buffers
    /// (silence when unconnected, unless written).
    fn set_input_connected(&mut self, _index: usize, _connected: bool) {}

    /// Ids of modules whose controls this module writes during
    /// [`Module::process`] (e.g. the control scheduler's targets).
    ///
    /// The signal graph treats these as ordering dependencies when compiling
    /// the process order, so a scheduled control write lands before its
    /// target processes the same block. A dependency that forms a cycle
    /// (e.g. scheduling the clock that drives the scheduler) folds both
    /// modules into a feedback group, processed sample-by-sample.
    ///
    /// Called on topology change, never on the audio hot path;
    /// implementations may briefly lock and allocate.
    fn control_targets(&self) -> Vec<String> {
        Vec::new()
    }

    /// Called once on the control thread after a newly built instance is
    /// attached and before it is published to the audio thread. Do here any
    /// one-time work the first [`Module::process`] would otherwise do on the
    /// audio thread, such as adopting shared state that needs a lock or an
    /// allocation. May lock and allocate. The default does nothing.
    fn prepare_for_publication(&mut self) {}

    /// This module's declared controls and the cells it publishes their
    /// values to, once it takes control requests. A module that
    /// returns `None` keeps the legacy `set_control` path.
    ///
    /// Crate-internal for now: first-party modules only, until the plugin
    /// interface can declare controls.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        None
    }

    /// Applies `value` to declared control `control` and returns the value
    /// the control now holds (after any clamping), or refuses it.
    ///
    /// Runs on the audio thread, between [`Module::process`] calls, at the
    /// sample the request is due, so the module needs no synchronization of
    /// its own and no sample offset: it sets its state as a plain setter. It
    /// must not allocate, free, lock or block. `value` is of the kind the
    /// table declares (a control thread coerced it), though a module still
    /// refuses one it cannot hold. Applying the value a control holds
    /// changes nothing, except for an event.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let _ = (control, value);
        Err(Refusal::Unsupported)
    }

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
mod config_fidelity_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod validation_tests;
