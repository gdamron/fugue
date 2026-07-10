//! ADSR envelope generator module.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::AdsrControls;

mod controls;
mod inputs;
mod outputs;

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
        let attack = config
            .get("attack")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.01) as f32;
        let decay = config.get("decay").and_then(|v| v.as_f64()).unwrap_or(0.1) as f32;
        let sustain = config
            .get("sustain")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7) as f32;
        let release = config
            .get("release")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.2) as f32;

        // Create controls with initial values
        let controls = AdsrControls::new(attack, decay, sustain, release);
        let adsr = Adsr::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(adsr)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
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
/// - `pedal`: Sustain-pedal gate (>0.0 = dampers up). While high, a gate-off
///   does not enter release: the note *rings*, continuing its natural decay
///   past the sustain level toward silence. When the pedal falls, a ringing
///   note enters release. A note whose gate is still high is unaffected.
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

    // Signal inputs (set each block when connected)
    inputs: inputs::AdsrInputs,

    // Cached output block
    outputs: outputs::AdsrOutputs,

    // State
    envelope_value: f32,
    last_gate_high: bool,
    last_pedal_high: bool,
    /// The gate has ended but the sustain pedal is holding the note: it keeps
    /// its natural decay (through the sustain level, toward silence) instead
    /// of entering release. Cleared by a new gate or by the pedal falling.
    ringing: bool,
    phase: EnvelopePhase,
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
            inputs: inputs::AdsrInputs::new(),
            outputs: outputs::AdsrOutputs::new(),
            envelope_value: 0.0,
            last_gate_high: false,
            last_pedal_high: false,
            ringing: false,
            phase: EnvelopePhase::Idle,
        }
    }

    /// Returns the effective attack time (signal or control) at frame `i`.
    fn effective_attack(&self, i: usize) -> f32 {
        self.inputs.attack(i, self.ctrl.attack())
    }

    /// Returns the effective decay time (signal or control) at frame `i`.
    fn effective_decay(&self, i: usize) -> f32 {
        self.inputs.decay(i, self.ctrl.decay())
    }

    /// Returns the effective sustain level (signal or control) at frame `i`.
    fn effective_sustain(&self, i: usize) -> f32 {
        self.inputs.sustain(i, self.ctrl.sustain())
    }

    /// Returns the effective release time (signal or control) at frame `i`.
    fn effective_release(&self, i: usize) -> f32 {
        self.inputs.release(i, self.ctrl.release())
    }

    /// Computes rate of change per sample for a given time duration.
    fn rate_per_sample(&self, time_seconds: f32) -> f32 {
        if time_seconds <= 0.0 {
            return 1.0; // Instant
        }
        1.0 / (time_seconds * self.sample_rate as f32)
    }

    /// Processes one sample of the envelope at frame `i`.
    fn process_envelope(&mut self, i: usize) {
        let gate_high = self.inputs.gate(i) > 0.0;
        let gate_triggered = gate_high && !self.last_gate_high;
        let gate_released = !gate_high && self.last_gate_high;
        let pedal_high = self.inputs.pedal(i) > 0.0;
        let pedal_released = !pedal_high && self.last_pedal_high;

        // State transitions
        if gate_triggered {
            self.phase = EnvelopePhase::Attack;
            self.ringing = false;
        } else if gate_released {
            if pedal_high {
                // Dampers are up: keep the natural decay instead of releasing.
                self.ringing = true;
            } else {
                self.phase = EnvelopePhase::Release;
            }
        }
        if pedal_released && self.ringing {
            self.phase = EnvelopePhase::Release;
            self.ringing = false;
        }

        // Get effective values
        let attack = self.effective_attack(i);
        let decay = self.effective_decay(i);
        let sustain = self.effective_sustain(i);
        let release = self.effective_release(i);
        // A ringing note decays past the sustain level toward silence.
        let decay_floor = if self.ringing { 0.0 } else { sustain };

        // Process based on phase
        match self.phase {
            EnvelopePhase::Idle => {
                self.envelope_value = 0.0;
                self.ringing = false;
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
                if self.envelope_value <= decay_floor {
                    self.envelope_value = decay_floor;
                    self.phase = if self.ringing {
                        EnvelopePhase::Idle
                    } else {
                        EnvelopePhase::Sustain
                    };
                }
            }
            EnvelopePhase::Sustain => {
                if self.ringing {
                    // The gate ended at the sustain plateau: resume the
                    // natural decay toward silence.
                    self.phase = EnvelopePhase::Decay;
                } else {
                    self.envelope_value = sustain;
                }
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
        self.last_pedal_high = pedal_high;
        self.outputs.set(i, self.envelope_value);
    }
}

impl Module for Adsr {
    fn name(&self) -> &str {
        "Adsr"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            self.process_envelope(i);
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
        adsr.process(1);
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_adsr_gate_triggers_attack() {
        let mut adsr = Adsr::new(44100);
        adsr.set_input("gate", 1.0).unwrap();
        adsr.process(1);
        assert!(adsr.get_output("envelope").unwrap() > 0.0);
    }

    #[test]
    fn test_adsr_instant_attack() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_input("gate", 1.0).unwrap();
        adsr.process(1);
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
            adsr.process(1);
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
            adsr.process(1);
        }

        // Now release
        adsr.set_input("gate", 0.0).unwrap();

        // Process release phase
        let release_samples = (0.01 * 44100.0) as usize + 10;
        for _ in 0..release_samples {
            adsr.process(1);
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

    /// Advances the envelope one sample at a time, so `get_output` (which
    /// reads frame 0 of the last block) reflects the final sample.
    fn run(adsr: &mut Adsr, samples: usize) {
        for _ in 0..samples {
            adsr.process(1);
        }
    }

    #[test]
    fn test_pedal_holds_natural_decay_past_gate_off() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 1.0).unwrap();
        adsr.set_control("sustain", 0.2).unwrap();
        adsr.set_control("release", 0.001).unwrap();

        // Strike with the pedal down, then lift the key almost immediately.
        adsr.set_input("pedal", 1.0).unwrap();
        adsr.set_input("gate", 1.0).unwrap();
        run(&mut adsr, 10);
        adsr.set_input("gate", 0.0).unwrap();

        // Long past the 0.001s release time, the note still rings...
        run(&mut adsr, 4410); // 0.1s
        let ringing = adsr.get_output("envelope").unwrap();
        assert!(
            ringing > 0.2,
            "pedal-held note should still ring well above silence, got {}",
            ringing
        );

        // ...and it decays *through* the sustain level toward silence.
        run(&mut adsr, 2 * 44100);
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_pedal_up_releases_ringing_note() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 10.0).unwrap();
        adsr.set_control("sustain", 0.2).unwrap();
        adsr.set_control("release", 0.01).unwrap();

        adsr.set_input("pedal", 1.0).unwrap();
        adsr.set_input("gate", 1.0).unwrap();
        run(&mut adsr, 10);
        adsr.set_input("gate", 0.0).unwrap();
        run(&mut adsr, 100);
        let before = adsr.get_output("envelope").unwrap();
        assert!(before > 0.9, "slow decay should still be near peak");

        // Pedal up: the ringing note releases (0.01s) instead of decaying
        // for the remaining ~10s.
        adsr.set_input("pedal", 0.0).unwrap();
        run(&mut adsr, (0.01 * 44100.0) as usize + 10);
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_pedal_does_not_affect_held_gate() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 0.0).unwrap();
        adsr.set_control("sustain", 0.5).unwrap();

        // Key held through a pedal up/down cycle: stays at sustain.
        adsr.set_input("gate", 1.0).unwrap();
        adsr.set_input("pedal", 1.0).unwrap();
        run(&mut adsr, 100);
        adsr.set_input("pedal", 0.0).unwrap();
        run(&mut adsr, 100);
        let envelope = adsr.get_output("envelope").unwrap();
        assert!((envelope - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_pedal_sustain_from_plateau_resumes_decay() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 0.01).unwrap();
        adsr.set_control("sustain", 0.5).unwrap();
        adsr.set_control("release", 10.0).unwrap();

        // Reach the sustain plateau with the key down.
        adsr.set_input("gate", 1.0).unwrap();
        run(&mut adsr, 1000);
        let plateau = adsr.get_output("envelope").unwrap();
        assert!((plateau - 0.5).abs() < 0.01);

        // Lift the key with the pedal down: the note leaves the plateau and
        // decays to silence at the (fast) decay rate, not the 10s release.
        adsr.set_input("pedal", 1.0).unwrap();
        run(&mut adsr, 1);
        adsr.set_input("gate", 0.0).unwrap();
        run(&mut adsr, 1000);
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_retrigger_while_ringing_clears_pedal_hold() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 10.0).unwrap();
        adsr.set_control("sustain", 0.2).unwrap();
        adsr.set_control("release", 0.01).unwrap();

        // Ring a note on the pedal, then strike again and release the key
        // after the pedal is already up: normal release applies.
        adsr.set_input("pedal", 1.0).unwrap();
        adsr.set_input("gate", 1.0).unwrap();
        run(&mut adsr, 10);
        adsr.set_input("gate", 0.0).unwrap();
        run(&mut adsr, 10);
        adsr.set_input("gate", 1.0).unwrap();
        run(&mut adsr, 10);
        adsr.set_input("pedal", 0.0).unwrap();
        run(&mut adsr, 10);
        let held = adsr.get_output("envelope").unwrap();
        assert!(held > 0.9, "retriggered note should be near peak");
        adsr.set_input("gate", 0.0).unwrap();
        run(&mut adsr, (0.01 * 44100.0) as usize + 10);
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_adsr_signal_overrides_control() {
        let mut adsr = Adsr::new(44100);

        // Set control to 0.5
        adsr.set_control("attack", 0.5).unwrap();

        // Signal input should override
        adsr.set_input("attack", 0.1).unwrap();
        assert_eq!(adsr.effective_attack(0), 0.1);

        // After disconnecting the attack port, should use control again
        adsr.set_input_connected(1, false);
        assert_eq!(adsr.effective_attack(0), 0.5);
    }
}

