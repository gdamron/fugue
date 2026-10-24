//! ADSR envelope generator module.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{ATTACK as ATTACK_CONTROL, DECAY as DECAY_CONTROL};
use self::controls::{RELEASE as RELEASE_CONTROL, SUSTAIN as SUSTAIN_CONTROL, TABLE};

mod controls;
mod inputs;
mod outputs;

/// Factory for constructing ADSR modules from configuration.
pub struct AdsrFactory;

const TYPE_ID: &str = "adsr";
const ATTACK: ConfigKey = ConfigKey::float("attack");
const DECAY: ConfigKey = ConfigKey::float("decay");
const SUSTAIN: ConfigKey = ConfigKey::float("sustain");
const RELEASE: ConfigKey = ConfigKey::float("release");

impl ModuleFactory for AdsrFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[ATTACK, DECAY, SUSTAIN, RELEASE]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // Parse config values with defaults
        let reader = ConfigReader::new(TYPE_ID, config);
        let attack = reader.float(&ATTACK)?.unwrap_or(0.01);
        let decay = reader.float(&DECAY)?.unwrap_or(0.1);
        let sustain = reader.float(&SUSTAIN)?.unwrap_or(0.7);
        let release = reader.float(&RELEASE)?.unwrap_or(0.2);

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(ATTACK_CONTROL, RtValue::F32(attack));
        cells.publish(DECAY_CONTROL, RtValue::F32(decay));
        cells.publish(SUSTAIN_CONTROL, RtValue::F32(sustain));
        cells.publish(RELEASE_CONTROL, RtValue::F32(release));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        let adsr = Adsr::with_cells(sample_rate, cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(adsr)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
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
///     {"from": "clock", "from_port": "beat", "to": "adsr", "to_port": "gate"},
///     {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "level"}
///   ]
/// }
/// ```
pub struct Adsr {
    sample_rate: u32,

    // Controls, applied on the thread running the envelope.
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cells: Arc<ControlCells>,

    // Signal inputs (set each block when connected)
    inputs: inputs::AdsrInputs,

    // Cached output block
    outputs: outputs::AdsrOutputs,

    // State
    envelope_value: f32,
    last_gate_high: bool,
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
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// An envelope holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut adsr = Self {
            sample_rate,
            attack: 0.0,
            decay: 0.0,
            sustain: 0.0,
            release: 0.0,
            cells,
            inputs: inputs::AdsrInputs::new(),
            outputs: outputs::AdsrOutputs::new(),
            envelope_value: 0.0,
            last_gate_high: false,
            phase: EnvelopePhase::Idle,
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = adsr.cells.load(index) {
                let _ = apply_declared(&mut adsr, index, value);
            }
        }
        adsr
    }

    /// Returns the effective attack time (signal or control) at frame `i`.
    #[cfg(test)]
    fn effective_attack(&self, i: usize) -> f32 {
        self.inputs.attack(i, self.attack)
    }

    /// Computes rate of change per sample for a given time duration.
    fn rate_per_sample(&self, time_seconds: f32) -> f32 {
        if time_seconds <= 0.0 {
            return 1.0; // Instant
        }
        1.0 / (time_seconds * self.sample_rate as f32)
    }

    /// Processes one sample using scalar controls captured for this process
    /// call. Gate and connected parameter inputs remain audio-rate.
    #[inline(always)]
    fn process_envelope_with(
        &mut self,
        i: usize,
        control_attack: f32,
        control_decay: f32,
        control_sustain: f32,
        control_release: f32,
    ) {
        let gate_high = self.inputs.gate(i) > 0.0;
        let gate_triggered = gate_high && !self.last_gate_high;
        let gate_released = !gate_high && self.last_gate_high;

        // State transitions
        if gate_triggered {
            self.phase = EnvelopePhase::Attack;
        } else if gate_released {
            self.phase = EnvelopePhase::Release;
        }

        // Get effective values
        let attack = self.inputs.attack(i, control_attack);
        let decay = self.inputs.decay(i, control_decay);
        let sustain = self.inputs.sustain(i, control_sustain);
        let release = self.inputs.release(i, control_release);

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
        self.outputs.set(i, self.envelope_value);
    }
}

impl Module for Adsr {
    fn name(&self) -> &str {
        "Adsr"
    }

    fn process(&mut self, frames: usize) -> bool {
        let control_attack = if self.inputs.attack_connected() {
            0.0
        } else {
            self.attack
        };
        let control_decay = if self.inputs.decay_connected() {
            0.0
        } else {
            self.decay
        };
        let control_sustain = if self.inputs.sustain_connected() {
            0.0
        } else {
            self.sustain
        };
        let control_release = if self.inputs.release_connected() {
            0.0
        } else {
            self.release
        };

        let mut i = 0;
        while i < frames {
            self.process_envelope_with(
                i,
                control_attack,
                control_decay,
                control_sustain,
                control_release,
            );
            i += 1;
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
        let RtValue::F32(value) = value else {
            return Err(Refusal::Unsupported);
        };
        let field = match control {
            ATTACK_CONTROL => &mut self.attack,
            DECAY_CONTROL => &mut self.decay,
            SUSTAIN_CONTROL => {
                self.sustain = value.clamp(0.0, 1.0);
                return Ok(RtValue::F32(self.sustain));
            }
            RELEASE_CONTROL => &mut self.release,
            _ => return Err(Refusal::Unsupported),
        };
        *field = value.max(0.0);
        Ok(RtValue::F32(*field))
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

    /// Renders `frames` with the gate held at `gate`, a block at a time.
    fn render(adsr: &mut Adsr, gate: f32, frames: usize) {
        for _ in 0..frames {
            adsr.input_block_mut(0)[0] = gate;
            adsr.process(1);
        }
    }

    #[test]
    fn test_changing_attack_mid_stage_keeps_the_level_and_changes_the_rate() {
        let mut adsr = Adsr::new(1000);
        adsr.set_control("attack", 1.0).unwrap();
        render(&mut adsr, 1.0, 100);
        let level = adsr.get_output("envelope").unwrap();
        assert!((level - 0.1).abs() < 1e-4, "{level}");

        // Twice as fast from here: the level carries on from where it was.
        adsr.set_control("attack", 0.5).unwrap();
        render(&mut adsr, 1.0, 1);
        let level = adsr.get_output("envelope").unwrap();
        assert!((level - 0.102).abs() < 1e-4, "{level}");
    }

    #[test]
    fn test_changing_release_mid_stage_keeps_the_level_and_changes_the_rate() {
        let mut adsr = Adsr::new(1000);
        adsr.set_control("attack", 0.0).unwrap();
        adsr.set_control("decay", 0.0).unwrap();
        adsr.set_control("sustain", 1.0).unwrap();
        adsr.set_control("release", 1.0).unwrap();
        render(&mut adsr, 1.0, 10);
        render(&mut adsr, 0.0, 100);
        let level = adsr.get_output("envelope").unwrap();
        assert!((level - 0.9).abs() < 1e-4, "{level}");

        adsr.set_control("release", 0.5).unwrap();
        render(&mut adsr, 0.0, 1);
        let level = adsr.get_output("envelope").unwrap();
        assert!((level - 0.898).abs() < 1e-4, "{level}");
    }

    #[test]
    fn test_controls_are_clamped_as_they_always_were() {
        let mut adsr = Adsr::new(44100);
        adsr.set_control("attack", -1.0).unwrap();
        adsr.set_control("sustain", 2.0).unwrap();
        assert_eq!(adsr.get_control("attack").unwrap(), 0.0);
        assert_eq!(adsr.get_control("sustain").unwrap(), 1.0);
        assert!(adsr.set_control("attack", f32::NAN).is_err());
    }
}
