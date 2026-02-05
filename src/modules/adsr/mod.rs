//! ADSR envelope generator module.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::AdsrControls;

mod controls;

/// Factory for constructing ADSR modules from configuration.
pub struct AdsrFactory;

impl ModuleFactory for AdsrFactory {
    fn type_id(&self) -> &'static str {
        "adsr"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // Parse config values with defaults
        let attack = config.get("attack").and_then(|v| v.as_f64()).unwrap_or(0.01) as f32;
        let decay = config.get("decay").and_then(|v| v.as_f64()).unwrap_or(0.1) as f32;
        let sustain = config.get("sustain").and_then(|v| v.as_f64()).unwrap_or(0.7) as f32;
        let release = config.get("release").and_then(|v| v.as_f64()).unwrap_or(0.2) as f32;

        // Create controls with initial values
        let controls = AdsrControls::new(attack, decay, sustain, release);
        let adsr = Adsr::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(adsr)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
            sink: None,
        })
    }
}

/// ADSR envelope generator with named ports for modular routing.
///
/// Generates classic Attack-Decay-Sustain-Release envelope curves based on
/// a gate input signal.
///
/// # Inputs
/// - `gate`: Trigger/gate signal (>0.0 = on, 0.0 = off)
/// - `attack`: Attack time in seconds (overrides control if connected)
/// - `decay`: Decay time in seconds (overrides control if connected)
/// - `sustain`: Sustain level 0.0-1.0 (overrides control if connected)
/// - `release`: Release time in seconds (overrides control if connected)
///
/// # Outputs
/// - `envelope`: Envelope value 0.0-1.0 suitable for VCA control
///
/// # Controls
/// - `attack`: Attack time in seconds (default: 0.01)
/// - `decay`: Decay time in seconds (default: 0.1)
/// - `sustain`: Sustain level 0.0-1.0 (default: 0.7)
/// - `release`: Release time in seconds (default: 0.2)
///
/// # Example
///
/// ```rust,ignore
/// // Route clock gate to ADSR, then to VCA
/// {
///   "connections": [
///     {"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"},
///     {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"}
///   ]
/// }
/// ```
pub struct Adsr {
    sample_rate: u32,

    // Controls (shared with AdsrControls handle)
    ctrl: AdsrControls,

    // Signal inputs (set each sample when connected)
    gate_in: f32,
    attack_in: f32,
    decay_in: f32,
    sustain_in: f32,
    release_in: f32,

    // Flags for whether signal was received this sample
    attack_active: bool,
    decay_active: bool,
    sustain_active: bool,
    release_active: bool,

    // State
    envelope_value: f32,
    last_gate_high: bool,
    phase: EnvelopePhase,
    last_processed_sample: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EnvelopePhase {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl Adsr {
    /// Creates a new ADSR envelope generator with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = AdsrControls::new(0.01, 0.1, 0.7, 0.2);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new ADSR envelope generator with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: AdsrControls) -> Self {
        Self {
            sample_rate,
            ctrl: controls,
            gate_in: 0.0,
            attack_in: 0.0,
            decay_in: 0.0,
            sustain_in: 0.0,
            release_in: 0.0,
            attack_active: false,
            decay_active: false,
            sustain_active: false,
            release_active: false,
            envelope_value: 0.0,
            last_gate_high: false,
            phase: EnvelopePhase::Idle,
            last_processed_sample: 0,
        }
    }

    /// Returns the effective attack time (signal or control).
    fn effective_attack(&self) -> f32 {
        if self.attack_active {
            self.attack_in
        } else {
            self.ctrl.attack()
        }
    }

    /// Returns the effective decay time (signal or control).
    fn effective_decay(&self) -> f32 {
        if self.decay_active {
            self.decay_in
        } else {
            self.ctrl.decay()
        }
    }

    /// Returns the effective sustain level (signal or control).
    fn effective_sustain(&self) -> f32 {
        if self.sustain_active {
            self.sustain_in
        } else {
            self.ctrl.sustain()
        }
    }

    /// Returns the effective release time (signal or control).
    fn effective_release(&self) -> f32 {
        if self.release_active {
            self.release_in
        } else {
            self.ctrl.release()
        }
    }

    /// Computes rate of change per sample for a given time duration.
    fn rate_per_sample(&self, time_seconds: f32) -> f32 {
        if time_seconds <= 0.0 {
            return 1.0; // Instant
        }
        1.0 / (time_seconds * self.sample_rate as f32)
    }

    /// Processes one sample of the envelope.
    fn process_envelope(&mut self) {
        let gate_high = self.gate_in > 0.0;
        let gate_triggered = gate_high && !self.last_gate_high;
        let gate_released = !gate_high && self.last_gate_high;

        // State transitions
        if gate_triggered {
            self.phase = EnvelopePhase::Attack;
        } else if gate_released {
            self.phase = EnvelopePhase::Release;
        }

        // Get effective values
        let attack = self.effective_attack();
        let decay = self.effective_decay();
        let sustain = self.effective_sustain();
        let release = self.effective_release();

        // Process based on phase
        match self.phase {
            EnvelopePhase::Idle => {
                self.envelope_value = 0.0;
            }
            EnvelopePhase::Attack => {
                let rate = self.rate_per_sample(attack);
                self.envelope_value += rate;
                if self.envelope_value >= 1.0 {
                    self.envelope_value = 1.0;
                    self.phase = EnvelopePhase::Decay;
                }
            }
            EnvelopePhase::Decay => {
                let rate = self.rate_per_sample(decay);
                self.envelope_value -= rate;
                if self.envelope_value <= sustain {
                    self.envelope_value = sustain;
                    self.phase = EnvelopePhase::Sustain;
                }
            }
            EnvelopePhase::Sustain => {
                self.envelope_value = sustain;
            }
            EnvelopePhase::Release => {
                let rate = self.rate_per_sample(release);
                self.envelope_value -= rate;
                if self.envelope_value <= 0.0 {
                    self.envelope_value = 0.0;
                    self.phase = EnvelopePhase::Idle;
                }
            }
        }

        self.last_gate_high = gate_high;
    }
}

impl Module for Adsr {
    fn name(&self) -> &str {
        "Adsr"
    }

    fn process(&mut self) -> bool {
        self.process_envelope();
        true
    }

    fn inputs(&self) -> &[&str] {
        &["gate", "attack", "decay", "sustain", "release"]
    }

    fn outputs(&self) -> &[&str] {
        &["envelope"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate_in = value;
                Ok(())
            }
            "attack" => {
                self.attack_in = value.max(0.0);
                self.attack_active = true;
                Ok(())
            }
            "decay" => {
                self.decay_in = value.max(0.0);
                self.decay_active = true;
                Ok(())
            }
            "sustain" => {
                self.sustain_in = value.clamp(0.0, 1.0);
                self.sustain_active = true;
                Ok(())
            }
            "release" => {
                self.release_in = value.max(0.0);
                self.release_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "envelope" => Ok(self.envelope_value.clamp(0.0, 1.0)),
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
        self.attack_active = false;
        self.decay_active = false;
        self.sustain_active = false;
        self.release_active = false;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("attack", "Attack time in seconds")
                .with_range(0.0, 10.0)
                .with_default(0.01),
            ControlMeta::new("decay", "Decay time in seconds")
                .with_range(0.0, 10.0)
                .with_default(0.1),
            ControlMeta::new("sustain", "Sustain level")
                .with_range(0.0, 1.0)
                .with_default(0.7),
            ControlMeta::new("release", "Release time in seconds")
                .with_range(0.0, 10.0)
                .with_default(0.2),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "attack" => Ok(self.ctrl.attack()),
            "decay" => Ok(self.ctrl.decay()),
            "sustain" => Ok(self.ctrl.sustain()),
            "release" => Ok(self.ctrl.release()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "attack" => {
                self.ctrl.set_attack(value);
                Ok(())
            }
            "decay" => {
                self.ctrl.set_decay(value);
                Ok(())
            }
            "sustain" => {
                self.ctrl.set_sustain(value);
                Ok(())
            }
            "release" => {
                self.ctrl.set_release(value);
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
    fn test_adsr_idle() {
        let mut adsr = Adsr::new(44100);
        adsr.process();
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_adsr_gate_triggers_attack() {
        let mut adsr = Adsr::new(44100);
        adsr.set_input("gate", 1.0).unwrap();
        adsr.process();
        assert!(adsr.get_output("envelope").unwrap() > 0.0);
    }

    #[test]
    fn test_adsr_instant_attack() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_input("gate", 1.0).unwrap();
        adsr.process();
        assert_eq!(adsr.get_output("envelope").unwrap(), 1.0);
    }

    #[test]
    fn test_adsr_sustain_level() {
        let mut adsr = Adsr::new(44100);

        // Set very short attack and decay via controls
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 0.0).unwrap();
        adsr.set_control("sustain", 0.5).unwrap();
        adsr.set_input("gate", 1.0).unwrap();

        // Process through attack and decay to reach sustain
        for _ in 0..10 {
            adsr.process();
        }

        let envelope = adsr.get_output("envelope").unwrap();
        assert!((envelope - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_adsr_release() {
        let mut adsr = Adsr::new(44100);

        // Get to sustain phase via controls
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 0.0).unwrap();
        adsr.set_control("sustain", 0.7).unwrap();
        adsr.set_control("release", 0.01).unwrap();
        adsr.set_input("gate", 1.0).unwrap();

        for _ in 0..10 {
            adsr.process();
        }

        // Now release
        adsr.set_input("gate", 0.0).unwrap();

        // Process release phase
        let release_samples = (0.01 * 44100.0) as usize + 10;
        for _ in 0..release_samples {
            adsr.process();
        }

        let envelope = adsr.get_output("envelope").unwrap();
        assert_eq!(envelope, 0.0);
    }

    #[test]
    fn test_adsr_invalid_ports() {
        let mut adsr = Adsr::new(44100);
        assert!(adsr.set_input("invalid", 0.5).is_err());
        assert!(adsr.get_output("invalid").is_err());
    }

    #[test]
    fn test_adsr_controls() {
        let mut adsr = Adsr::new(44100);

        // Test control metadata
        let controls = adsr.controls();
        assert_eq!(controls.len(), 4);
        assert_eq!(controls[0].key, "attack");
        assert_eq!(controls[1].key, "decay");
        assert_eq!(controls[2].key, "sustain");
        assert_eq!(controls[3].key, "release");

        // Test get/set controls
        adsr.set_control("attack", 0.5).unwrap();
        assert_eq!(adsr.get_control("attack").unwrap(), 0.5);

        adsr.set_control("sustain", 0.8).unwrap();
        assert_eq!(adsr.get_control("sustain").unwrap(), 0.8);

        // Test invalid control
        assert!(adsr.get_control("invalid").is_err());
        assert!(adsr.set_control("invalid", 0.5).is_err());
    }

    #[test]
    fn test_adsr_signal_overrides_control() {
        let mut adsr = Adsr::new(44100);

        // Set control to 0.5
        adsr.set_control("attack", 0.5).unwrap();

        // Signal input should override
        adsr.set_input("attack", 0.1).unwrap();
        assert!(adsr.attack_active);

        // After reset_inputs, should use control again
        adsr.reset_inputs();
        assert!(!adsr.attack_active);
    }
}
