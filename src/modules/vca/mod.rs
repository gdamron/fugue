//! Voltage Controlled Amplifier (VCA) module.
//!
//! A VCA multiplies an audio signal by a control voltage, allowing dynamic
//! amplitude control. Common uses include applying envelope shapes to sounds,
//! tremolo effects, and level control.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::LinearRamp;
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{LEVEL, TABLE};

mod controls;
mod inputs;
mod outputs;

/// The rate a VCA built without one glides at.
pub const DEFAULT_RATE: u32 = 48_000;

/// Factory for constructing VCA modules from configuration.
pub struct VcaFactory;

const TYPE_ID: &str = "vca";
const LEVEL_KEY: ConfigKey = ConfigKey::float("level");

impl ModuleFactory for VcaFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[LEVEL_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let level = ConfigReader::new(TYPE_ID, config)
            .float(&LEVEL_KEY)?
            .unwrap_or(1.0);
        let vca = Vca::with_level_at(level, sample_rate);
        let surface = DeclaredSurface::new(TABLE.clone(), vca.cells.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(vca)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A Voltage Controlled Amplifier that multiplies audio by a control voltage.
///
/// # Inputs
/// - `audio`: The audio signal to be amplified (typically -1.0 to 1.0)
/// - `level`: Amplitude (0.0 to 1.0, where 1.0 = full volume)
///
/// # Outputs
/// - `audio`: The amplified audio signal (audio * level)
///
/// # Controls
/// - `level`: Default level used when no level signal is connected (0.0-1.0)
///
/// # Example
///
/// ```rust,ignore
/// // Connect an envelope to control a VCA
/// // In invention JSON:
/// {
///   "connections": [
///     {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
///     {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "level"},
///     {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
///   ]
/// }
/// ```
pub struct Vca {
    /// Amplitude when no level signal is connected, 0 to 1. A write glides
    /// to its value, as a mixer's fader does: a step would click.
    level: LinearRamp,
    cells: Arc<ControlCells>,
    inputs: inputs::VcaInputs,
    outputs: outputs::VcaOutputs,
}

impl Vca {
    /// Creates a new VCA with level 1.0 (unity gain/passthrough).
    pub fn new() -> Self {
        Self::with_level(1.0)
    }

    /// Creates a new VCA at `level` (clamped to 0 to 1), its level writes
    /// gliding as they would at [`DEFAULT_RATE`].
    pub fn with_level(level: f32) -> Self {
        Self::with_level_at(level, DEFAULT_RATE)
    }

    /// Creates a new VCA at `level` (clamped to 0 to 1) running at
    /// `sample_rate`.
    pub fn with_level_at(level: f32, sample_rate: u32) -> Self {
        let mut vca = Self {
            level: LinearRamp::new(1.0, gain_ramp_frames(sample_rate)),
            cells: Arc::new(ControlCells::new([RtValue::F32(1.0)])),
            inputs: inputs::VcaInputs::new(),
            outputs: outputs::VcaOutputs::new(),
        };
        let _ = apply_declared(&mut vca, LEVEL, RtValue::F32(level));
        // The configured level is where it starts, not a glide.
        if let Some(RtValue::F32(level)) = vca.cells.load(LEVEL) {
            vca.level.snap(level);
        }
        vca
    }
}

impl Default for Vca {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Vca {
    fn name(&self) -> &str {
        "Vca"
    }

    fn process(&mut self, frames: usize) -> bool {
        if self.inputs.cv_connected() {
            for i in 0..frames {
                let value = self.inputs.audio(i) * self.inputs.cv(i, 0.0);
                self.outputs.set(i, value);
            }
        } else if self.level.is_gliding() {
            for i in 0..frames {
                let value = self.inputs.audio(i) * self.level.next_value();
                self.outputs.set(i, value);
            }
        } else {
            let level = self.level.value();
            for i in 0..frames {
                let value = self.inputs.audio(i) * level;
                self.outputs.set(i, value);
            }
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (LEVEL, RtValue::F32(level)) => {
                let level = level.clamp(0.0, 1.0);
                self.level.set_target(level);
                Ok(RtValue::F32(level))
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vca_basic() {
        let mut vca = Vca::new();

        // Full volume (default)
        vca.set_input("audio", 0.5).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);

        // Half volume via CV signal
        vca.set_input("level", 0.5).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.25);

        // Silence
        vca.set_input("level", 0.0).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }

    #[test]
    fn test_vca_cv_clamping() {
        let mut vca = Vca::new();

        vca.set_input("audio", 1.0).unwrap();

        // CV above 1.0 should be clamped
        vca.set_input("level", 2.0).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 1.0);

        // CV below 0.0 should be clamped
        vca.set_input("level", -0.5).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }

    #[test]
    fn test_vca_invalid_ports() {
        let mut vca = Vca::new();

        assert!(vca.set_input("invalid", 0.5).is_err());
        assert!(vca.get_output("invalid").is_err());
    }

    #[test]
    fn test_vca_controls() {
        let mut vca = Vca::new();

        // Test control metadata
        let control_meta = Module::controls(&vca);
        assert_eq!(control_meta.len(), 1);
        assert_eq!(control_meta[0].key, "level");

        // Test get/set controls
        vca.set_control("level", 0.5).unwrap();
        assert_eq!(vca.get_control("level").unwrap(), 0.5);

        // Test invalid control
        assert!(vca.get_control("invalid").is_err());
    }

    #[test]
    fn test_vca_signal_overrides_control() {
        let mut vca = Vca::new();

        // Set control CV
        vca.set_control("level", 0.5).unwrap();

        vca.set_input("audio", 1.0).unwrap();

        // Without a connected level signal, should use control, once its
        // write has glided there
        vca.set_input_connected(1, false);
        for _ in 0..gain_ramp_frames(DEFAULT_RATE) {
            vca.process(1);
        }
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);

        // With signal, should use signal
        vca.set_input("level", 0.25).unwrap();
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.25);

        // After disconnecting, should use control again
        vca.set_input_connected(1, false);
        vca.process(1);
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);
    }

    #[test]
    fn test_connected_cv_remains_audio_rate() {
        let mut vca = Vca::new();
        vca.input_block_mut(0)[..3].copy_from_slice(&[1.0, 0.5, -1.0]);
        vca.input_block_mut(1)[..3].copy_from_slice(&[0.0, 0.5, 1.0]);
        vca.set_input_connected(1, true);

        vca.process(3);

        assert_eq!(&vca.output_block(0)[..3], &[0.0, 0.25, -1.0]);
    }

    #[test]
    fn a_level_write_glides_without_a_step_or_an_allocation() {
        use crate::alloc_counter::allocator_events;
        use crate::audio_thread::AudioThreadScope;
        let mut vca = Vca::with_level_at(1.0, 44_100);
        let glide = gain_ramp_frames(44_100) as usize;
        vca.input_block_mut(0)[..glide].fill(1.0);
        vca.set_control("level", 0.0).unwrap();
        let ((), allocs, frees) = allocator_events(|| {
            let _audio = AudioThreadScope::enter();
            vca.process(glide);
        });
        assert_eq!((allocs, frees), (0, 0));
        let heard = &vca.output_block(0)[..glide];
        let step = 1.0 / glide as f32;
        let mut previous = 1.0;
        for (frame, &value) in heard.iter().enumerate() {
            assert!(
                (previous - value - step).abs() < step * 0.01,
                "frame {frame}: {previous} -> {value}"
            );
            previous = value;
        }
        assert_eq!(heard[glide - 1], 0.0, "it lands on the level");
    }

    #[test]
    fn a_configured_level_holds_from_the_first_frame() {
        let mut vca = Vca::with_level_at(0.25, 44_100);
        vca.input_block_mut(0)[0] = 1.0;
        vca.process(1);
        assert_eq!(vca.output_block(0)[0], 0.25);
    }
}
