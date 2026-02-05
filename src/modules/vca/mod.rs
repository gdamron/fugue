//! Voltage Controlled Amplifier (VCA) module.
//!
//! A VCA multiplies an audio signal by a control voltage, allowing dynamic
//! amplitude control. Common uses include applying envelope shapes to sounds,
//! tremolo effects, and level control.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::VcaControls;

mod controls;

/// Factory for constructing VCA modules from configuration.
pub struct VcaFactory;

impl ModuleFactory for VcaFactory {
    fn type_id(&self) -> &'static str {
        "vca"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let cv = config
            .get("cv")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;

        let controls = VcaControls::new(cv);
        let vca = Vca::new_with_controls(controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(vca)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
            sink: None,
        })
    }
}

/// A Voltage Controlled Amplifier that multiplies audio by a control voltage.
///
/// # Inputs
/// - `audio`: The audio signal to be amplified (typically -1.0 to 1.0)
/// - `cv`: Control voltage for amplitude (0.0 to 1.0, where 1.0 = full volume)
///
/// # Outputs
/// - `audio`: The amplified audio signal (audio * cv)
///
/// # Controls
/// - `cv`: Default CV value used when no cv signal is connected (0.0-1.0)
///
/// # Example
///
/// ```rust,ignore
/// // Connect an envelope to control a VCA
/// // In patch JSON:
/// {
///   "connections": [
///     {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
///     {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
///     {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
///   ]
/// }
/// ```
pub struct Vca {
    ctrl: VcaControls,
    audio_in: f32,
    cv_in: f32,
    cv_active: bool,
    last_processed_sample: u64, // For pull-based processing
}

impl Vca {
    /// Creates a new VCA with CV defaulting to 1.0 (unity gain/passthrough).
    pub fn new() -> Self {
        Self::new_with_controls(VcaControls::default())
    }

    /// Creates a new VCA with the given controls.
    pub fn new_with_controls(controls: VcaControls) -> Self {
        Self {
            ctrl: controls,
            audio_in: 0.0,
            cv_in: 1.0,
            cv_active: false,
            last_processed_sample: 0,
        }
    }

    /// Returns the effective CV (signal or control).
    fn effective_cv(&self) -> f32 {
        if self.cv_active {
            self.cv_in
        } else {
            self.ctrl.cv()
        }
    }

    /// Returns a reference to the controls.
    pub fn controls(&self) -> &VcaControls {
        &self.ctrl
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

    fn process(&mut self) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio", "cv"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio_in = value;
                Ok(())
            }
            "cv" => {
                self.cv_in = value.clamp(0.0, 1.0);
                self.cv_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio_in * self.effective_cv()),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn reset_inputs(&mut self) {
        self.cv_active = false;
        // audio_in doesn't have a control fallback
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![ControlMeta::new("cv", "Default CV level (when no signal connected)")
            .with_range(0.0, 1.0)
            .with_default(1.0)]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "cv" => Ok(self.ctrl.cv()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "cv" => {
                self.ctrl.set_cv(value);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
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
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);

        // Half volume via CV signal
        vca.set_input("cv", 0.5).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.25);

        // Silence
        vca.set_input("cv", 0.0).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }

    #[test]
    fn test_vca_cv_clamping() {
        let mut vca = Vca::new();

        vca.set_input("audio", 1.0).unwrap();

        // CV above 1.0 should be clamped
        vca.set_input("cv", 2.0).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 1.0);

        // CV below 0.0 should be clamped
        vca.set_input("cv", -0.5).unwrap();
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
        assert_eq!(control_meta[0].key, "cv");

        // Test get/set controls
        vca.set_control("cv", 0.5).unwrap();
        assert_eq!(vca.get_control("cv").unwrap(), 0.5);

        // Test invalid control
        assert!(vca.get_control("invalid").is_err());
    }

    #[test]
    fn test_vca_signal_overrides_control() {
        let mut vca = Vca::new();

        // Set control CV
        vca.set_control("cv", 0.5).unwrap();

        vca.set_input("audio", 1.0).unwrap();

        // Without signal, should use control
        vca.reset_inputs();
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);

        // With signal, should use signal
        vca.set_input("cv", 0.25).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.25);

        // After reset, should use control again
        vca.reset_inputs();
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);
    }
}
