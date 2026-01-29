//! Modular ADSR envelope generator with named ports.
//!
//! This module provides a simplified ADSR envelope that uses the named port system,
//! allowing flexible routing of gate signals and envelope outputs.

use crate::module::{ModularModule, Module};

/// ADSR envelope generator with named ports for modular routing.
///
/// Generates classic Attack-Decay-Sustain-Release envelope curves based on
/// a gate input signal. Unlike the old type-based ADSR, this version works
/// with simple f32 signals and can be connected to any gate source.
///
/// # Inputs
/// - `gate`: Trigger/gate signal (>0.0 = on, 0.0 = off)
/// - `attack`: Attack time in seconds (default: 0.01)
/// - `decay`: Decay time in seconds (default: 0.1)
/// - `sustain`: Sustain level 0.0-1.0 (default: 0.7)
/// - `release`: Release time in seconds (default: 0.2)
///
/// # Outputs
/// - `envelope`: Envelope value 0.0-1.0 suitable for VCA control
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
pub struct ModularAdsr {
    sample_rate: u32,
    // Inputs
    gate_in: f32,
    attack_in: f32,
    decay_in: f32,
    sustain_in: f32,
    release_in: f32,
    // State
    envelope_value: f32,
    last_gate_high: bool,
    phase: EnvelopePhase,
    last_processed_sample: u64, // For pull-based processing
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EnvelopePhase {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl ModularAdsr {
    /// Creates a new modular ADSR envelope generator.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            gate_in: 0.0,
            attack_in: 0.01,
            decay_in: 0.1,
            sustain_in: 0.7,
            release_in: 0.2,
            envelope_value: 0.0,
            last_gate_high: false,
            phase: EnvelopePhase::Idle,
            last_processed_sample: 0,
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

        // Process based on phase
        match self.phase {
            EnvelopePhase::Idle => {
                self.envelope_value = 0.0;
            }
            EnvelopePhase::Attack => {
                let rate = self.rate_per_sample(self.attack_in);
                self.envelope_value += rate;
                if self.envelope_value >= 1.0 {
                    self.envelope_value = 1.0;
                    self.phase = EnvelopePhase::Decay;
                }
            }
            EnvelopePhase::Decay => {
                let rate = self.rate_per_sample(self.decay_in);
                self.envelope_value -= rate;
                if self.envelope_value <= self.sustain_in {
                    self.envelope_value = self.sustain_in;
                    self.phase = EnvelopePhase::Sustain;
                }
            }
            EnvelopePhase::Sustain => {
                self.envelope_value = self.sustain_in;
            }
            EnvelopePhase::Release => {
                let rate = self.rate_per_sample(self.release_in);
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

impl Module for ModularAdsr {
    fn process(&mut self) -> bool {
        self.process_envelope();
        true
    }

    fn name(&self) -> &str {
        "ModularAdsr"
    }
}

impl ModularModule for ModularAdsr {
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
                Ok(())
            }
            "decay" => {
                self.decay_in = value.max(0.0);
                Ok(())
            }
            "sustain" => {
                self.sustain_in = value.clamp(0.0, 1.0);
                Ok(())
            }
            "release" => {
                self.release_in = value.max(0.0);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&mut self, port: &str) -> Result<f32, String> {
        match port {
            "envelope" => Ok(self.envelope_value.clamp(0.0, 1.0)),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn reset_inputs(&mut self) {
        self.gate_in = 0.0;
        // Don't reset ADSR parameters - they should retain their values
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn get_cached_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "envelope" => Ok(self.envelope_value.clamp(0.0, 1.0)),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modular_adsr_idle() {
        let mut adsr = ModularAdsr::new(44100);
        adsr.process();
        assert_eq!(adsr.get_output("envelope").unwrap(), 0.0);
    }

    #[test]
    fn test_modular_adsr_gate_triggers_attack() {
        let mut adsr = ModularAdsr::new(44100);
        adsr.set_input("gate", 1.0).unwrap();
        adsr.process();
        assert!(adsr.get_output("envelope").unwrap() > 0.0);
    }

    #[test]
    fn test_modular_adsr_instant_attack() {
        let mut adsr = ModularAdsr::new(44100);
        adsr.set_input("gate", 1.0).unwrap();
        adsr.set_input("attack", 0.0).unwrap();
        adsr.process();
        assert_eq!(adsr.get_output("envelope").unwrap(), 1.0);
    }

    #[test]
    fn test_modular_adsr_sustain_level() {
        let mut adsr = ModularAdsr::new(44100);

        // Set very short attack and decay
        adsr.set_input("attack", 0.0).unwrap();
        adsr.set_input("decay", 0.0).unwrap();
        adsr.set_input("sustain", 0.5).unwrap();
        adsr.set_input("gate", 1.0).unwrap();

        // Process through attack and decay to reach sustain
        for _ in 0..10 {
            adsr.process();
        }

        let envelope = adsr.get_output("envelope").unwrap();
        assert!((envelope - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_modular_adsr_release() {
        let mut adsr = ModularAdsr::new(44100);

        // Get to sustain phase
        adsr.set_input("attack", 0.0).unwrap();
        adsr.set_input("decay", 0.0).unwrap();
        adsr.set_input("sustain", 0.7).unwrap();
        adsr.set_input("release", 0.01).unwrap();
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
    fn test_modular_adsr_invalid_ports() {
        let mut adsr = ModularAdsr::new(44100);
        assert!(adsr.set_input("invalid", 0.5).is_err());
        assert!(adsr.get_output("invalid").is_err());
    }
}
