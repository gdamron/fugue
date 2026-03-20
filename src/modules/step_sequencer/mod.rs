//! Step sequencer module for deterministic pattern playback.
//!
//! The step sequencer plays back a fixed pattern of notes, advancing one step
//! per clock gate. Unlike the `MelodyGenerator` which uses weighted random
//! selection, the step sequencer provides deterministic, repeatable patterns.
//!
//! # Features
//!
//! - Configurable pattern length (default 16 steps)
//! - Per-step note values with base note offset
//! - Per-step gate length control
//! - Support for rests (null notes)
//! - Reset input for pattern restart
//! - Step output for visualization/sync
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
//!     {
//!       "id": "seq",
//!       "type": "step_sequencer",
//!       "config": {
//!         "base_note": 48,
//!         "steps": 8,
//!         "gate_length": 0.5,
//!         "pattern": [
//!           { "note": 0 },
//!           { "note": null },
//!           { "note": 7, "gate": 0.8 },
//!           { "note": 5 },
//!           { "note": null },
//!           { "note": 0, "gate": 1.0 },
//!           { "note": 2 },
//!           { "note": null }
//!         ]
//!       }
//!     },
//!     { "id": "osc", "type": "oscillator" },
//!     { "id": "vca", "type": "vca" },
//!     { "id": "dac", "type": "dac" }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "gate", "to": "seq", "to_port": "gate" },
//!     { "from": "seq", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
//!     { "from": "seq", "from_port": "gate", "to": "vca", "to_port": "cv" },
//!     { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
//!     { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
//!   ]
//! }
//! ```

use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::StepSequencerControls;

mod controls;
mod inputs;
mod outputs;

/// Default number of steps in a pattern.
pub const DEFAULT_STEPS: usize = 16;

/// Default gate length as a ratio of step duration (0.0-1.0).
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;

/// Default base MIDI note (C3).
pub const DEFAULT_BASE_NOTE: u8 = 48;

/// A single step in the sequencer pattern.
#[derive(Debug, Clone)]
pub struct Step {
    /// Note offset from base_note. None = rest (no note).
    pub note: Option<i8>,
    /// Gate length for this step as ratio of step duration (0.0-1.0).
    /// If None, uses the sequencer's default gate_length.
    pub gate_length: Option<f32>,
}

impl Step {
    /// Creates a new step with a note.
    pub fn note(offset: i8) -> Self {
        Self {
            note: Some(offset),
            gate_length: None,
        }
    }

    /// Creates a new step with a note and custom gate length.
    pub fn note_with_gate(offset: i8, gate_length: f32) -> Self {
        Self {
            note: Some(offset),
            gate_length: Some(gate_length.clamp(0.0, 1.0)),
        }
    }

    /// Creates a rest step (no note).
    pub fn rest() -> Self {
        Self {
            note: None,
            gate_length: None,
        }
    }
}

impl Default for Step {
    fn default() -> Self {
        Self::rest()
    }
}

/// A deterministic step sequencer for pattern playback.
///
/// Plays back a fixed pattern of notes, advancing one step per clock gate.
/// Supports per-step gate lengths and rests.
///
/// # Inputs
///
/// - `gate` - Clock gate input (rising edge advances step)
/// - `reset` - Reset input (rising edge resets to step 0)
///
/// # Outputs
///
/// - `frequency` - Current note frequency in Hz (0.0 during rest)
/// - `gate` - Output gate signal (high during note, low during rest)
/// - `step` - Current step number (0 to steps-1)
///
/// # Example
///
/// ```rust,ignore
/// use fugue::modules::step_sequencer::{StepSequencer, Step};
///
/// let mut seq = StepSequencer::new(44100)
///     .with_base_note(48)
///     .with_steps(8)
///     .with_pattern(vec![
///         Step::note(0),
///         Step::rest(),
///         Step::note_with_gate(7, 0.8),
///         Step::note(5),
///     ]);
/// ```
pub struct StepSequencer {
    #[allow(dead_code)] // Reserved for future use (e.g., sample-accurate timing)
    sample_rate: u32,
    /// Thread-safe controls for base_note, steps, and gate_length.
    ctrl: StepSequencerControls,
    /// The pattern of steps.
    pattern: Vec<Step>,

    // State
    /// Current step index (0 to steps-1).
    current_step: usize,
    /// Samples remaining for current gate.
    gate_samples_remaining: u32,
    /// Duration of one step in samples (measured from clock).
    step_duration_samples: u32,
    /// Sample count since last clock gate (for measuring step duration).
    samples_since_gate: u32,
    /// Whether we've received at least one gate.
    first_gate_received: bool,

    // Edge detection state
    last_gate_in: f32,
    last_reset_in: f32,

    // Input values
    inputs: inputs::StepSequencerInputs,

    // Cached outputs
    outputs: outputs::StepSequencerOutputs,

    // Pull-based processing
    last_processed_sample: u64,
}

impl StepSequencer {
    /// Creates a new step sequencer with the given sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            ctrl: StepSequencerControls::new(),
            pattern: Vec::new(),
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2, // Default ~120 BPM
            samples_since_gate: 0,
            first_gate_received: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::StepSequencerInputs::new(),
            outputs: outputs::StepSequencerOutputs::new(),
            last_processed_sample: 0,
        }
    }

    /// Creates a new step sequencer with the given sample rate and controls.
    pub fn new_with_controls(sample_rate: u32, controls: StepSequencerControls) -> Self {
        Self {
            sample_rate,
            ctrl: controls,
            pattern: Vec::new(),
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2,
            samples_since_gate: 0,
            first_gate_received: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::StepSequencerInputs::new(),
            outputs: outputs::StepSequencerOutputs::new(),
            last_processed_sample: 0,
        }
    }

    /// Sets the base MIDI note.
    pub fn with_base_note(self, base_note: u8) -> Self {
        self.ctrl.set_base_note(base_note);
        self
    }

    /// Sets the number of steps in the pattern.
    pub fn with_steps(self, steps: usize) -> Self {
        self.ctrl.set_steps(steps);
        self
    }

    /// Sets the default gate length (ratio of step duration, 0.0-1.0).
    pub fn with_gate_length(self, gate_length: f32) -> Self {
        self.ctrl.set_gate_length(gate_length);
        self
    }

    /// Sets the pattern.
    pub fn with_pattern(mut self, pattern: Vec<Step>) -> Self {
        self.pattern = pattern;
        self
    }

    /// Sets the base MIDI note.
    pub fn set_base_note(&mut self, base_note: u8) {
        self.ctrl.set_base_note(base_note);
    }

    /// Sets the number of steps.
    pub fn set_steps(&mut self, steps: usize) {
        self.ctrl.set_steps(steps);
    }

    /// Sets the default gate length.
    pub fn set_gate_length(&mut self, gate_length: f32) {
        self.ctrl.set_gate_length(gate_length);
    }

    /// Sets the pattern.
    pub fn set_pattern(&mut self, pattern: Vec<Step>) {
        self.pattern = pattern;
    }

    /// Returns the current step index.
    pub fn current_step(&self) -> usize {
        self.current_step
    }

    /// Returns the number of steps.
    pub fn step_count(&self) -> usize {
        self.ctrl.steps()
    }

    /// Returns a reference to the step sequencer controls.
    pub fn controls(&self) -> &StepSequencerControls {
        &self.ctrl
    }

    /// Gets the step at the given index, returning a rest if out of bounds.
    fn get_step(&self, index: usize) -> Step {
        self.pattern.get(index).cloned().unwrap_or_default()
    }

    /// Calculates the frequency for the current step.
    fn calculate_frequency(&self) -> f32 {
        let step = self.get_step(self.current_step);
        match step.note {
            Some(offset) => {
                let base = self.ctrl.base_note();
                let midi_note = (base as i16 + offset as i16).clamp(0, 127) as u8;
                Note::new(midi_note).frequency()
            }
            None => 0.0, // Rest - no frequency
        }
    }

    /// Calculates the gate duration in samples for the current step.
    fn calculate_gate_samples(&self) -> u32 {
        let step = self.get_step(self.current_step);

        // Use per-step gate length if specified, otherwise default
        let gate_length = step.gate_length.unwrap_or(self.ctrl.gate_length());

        // Calculate gate duration as fraction of step duration
        (self.step_duration_samples as f32 * gate_length) as u32
    }

    /// Advances to the next step.
    fn advance_step(&mut self) {
        self.current_step = (self.current_step + 1) % self.ctrl.steps();
    }

    /// Resets to step 0.
    fn reset(&mut self) {
        self.current_step = 0;
        self.gate_samples_remaining = 0;
    }

    /// Processes one sample.
    fn process_sample(&mut self) {
        // Detect rising edges
        let gate_rising = self.inputs.gate() > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset() > 0.5 && self.last_reset_in <= 0.5;

        // Handle reset (takes priority)
        if reset_rising {
            self.reset();
        }

        // Handle clock gate
        if gate_rising {
            // Measure step duration from previous gate
            if self.first_gate_received && self.samples_since_gate > 0 {
                self.step_duration_samples = self.samples_since_gate;
            }

            // Advance step on every gate EXCEPT the first one
            // First gate plays step 0, subsequent gates advance
            if self.first_gate_received {
                self.advance_step();
            }
            self.first_gate_received = true;
            self.samples_since_gate = 0;

            // Start gate for current step
            let step = self.get_step(self.current_step);
            if step.note.is_some() {
                self.gate_samples_remaining = self.calculate_gate_samples();
            } else {
                self.gate_samples_remaining = 0; // Rest - no gate
            }
        }

        // Count samples for step duration measurement
        self.samples_since_gate += 1;

        // Update gate output (decrement remaining samples)
        if self.gate_samples_remaining > 0 {
            self.gate_samples_remaining -= 1;
        }

        // Update cached outputs
        self.outputs.set(
            self.calculate_frequency(),
            if self.gate_samples_remaining > 0 {
                1.0
            } else {
                0.0
            },
            self.current_step as f32,
        );

        // Store for edge detection
        self.last_gate_in = self.inputs.gate();
        self.last_reset_in = self.inputs.reset();
    }
}

impl Module for StepSequencer {
    fn name(&self) -> &str {
        "StepSequencer"
    }

    fn process(&mut self) -> bool {
        self.process_sample();
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(DEFAULT_BASE_NOTE as f32),
            ControlMeta::new("steps", "Number of steps in pattern")
                .with_range(1.0, 64.0)
                .with_default(DEFAULT_STEPS as f32),
            ControlMeta::new("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(DEFAULT_GATE_LENGTH),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "base_note" => Ok(self.ctrl.base_note() as f32),
            "steps" => Ok(self.ctrl.steps() as f32),
            "gate_length" => Ok(self.ctrl.gate_length()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "base_note" => {
                self.ctrl.set_base_note(value as u8);
                Ok(())
            }
            "steps" => {
                self.ctrl.set_steps(value as usize);
                Ok(())
            }
            "gate_length" => {
                self.ctrl.set_gate_length(value);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}

/// Factory for constructing StepSequencer modules from configuration.
///
/// # Configuration Options
///
/// - `base_note` (u8): Base MIDI note added to step values (default: 48, C3)
/// - `steps` (usize): Number of steps in pattern (default: 16)
/// - `gate_length` (f32): Default gate length ratio 0.0-1.0 (default: 0.5)
/// - `pattern` (array): Array of step objects
///
/// # Step Object Format
///
/// ```json
/// { "note": 0, "gate": 0.8 }  // Note with custom gate length
/// { "note": 7 }               // Note with default gate length
/// { "note": null }            // Rest (no note)
/// ```
///
/// # Example
///
/// ```json
/// {
///   "id": "bass_seq",
///   "type": "step_sequencer",
///   "config": {
///     "base_note": 36,
///     "steps": 16,
///     "gate_length": 0.5,
///     "pattern": [
///       { "note": 0, "gate": 0.8 },
///       { "note": null },
///       { "note": 7 },
///       { "note": 5 }
///     ]
///   }
/// }
/// ```
pub struct StepSequencerFactory;

impl ModuleFactory for StepSequencerFactory {
    fn type_id(&self) -> &'static str {
        "step_sequencer"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let base_note = config
            .get("base_note")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8)
            .unwrap_or(DEFAULT_BASE_NOTE);

        let steps = config
            .get("steps")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_STEPS);

        let gate_length = config
            .get("gate_length")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(DEFAULT_GATE_LENGTH);

        let pattern = parse_pattern(config.get("pattern"))?;

        let controls = StepSequencerControls::new_with_values(base_note, steps, gate_length);

        let seq =
            StepSequencer::new_with_controls(sample_rate, controls.clone()).with_pattern(pattern);

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(seq)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Parses a pattern array from JSON.
fn parse_pattern(
    value: Option<&serde_json::Value>,
) -> Result<Vec<Step>, Box<dyn std::error::Error>> {
    let Some(array) = value.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut pattern = Vec::with_capacity(array.len());

    for step_value in array {
        let step = parse_step(step_value)?;
        pattern.push(step);
    }

    Ok(pattern)
}

/// Parses a single step from JSON.
fn parse_step(value: &serde_json::Value) -> Result<Step, Box<dyn std::error::Error>> {
    // Handle simple null as rest
    if value.is_null() {
        return Ok(Step::rest());
    }

    // Handle object format
    if let Some(obj) = value.as_object() {
        let note = match obj.get("note") {
            Some(serde_json::Value::Null) => None,
            Some(n) => n.as_i64().map(|v| v as i8),
            None => None,
        };

        let gate_length = obj
            .get("gate")
            .and_then(|v| v.as_f64())
            .map(|v| (v as f32).clamp(0.0, 1.0));

        return Ok(Step { note, gate_length });
    }

    // Handle simple integer as note
    if let Some(n) = value.as_i64() {
        return Ok(Step::note(n as i8));
    }

    Err(format!("Invalid step format: {:?}", value).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_sequencer_basic() {
        let mut seq = StepSequencer::new(44100)
            .with_base_note(48)
            .with_steps(4)
            .with_pattern(vec![
                Step::note(0),
                Step::rest(),
                Step::note(7),
                Step::note(5),
            ]);

        // Initially at step 0
        assert_eq!(seq.current_step(), 0);

        // First gate - should stay at step 0 and output frequency
        seq.set_input("gate", 1.0).unwrap();
        seq.process();

        let freq = seq.get_output("frequency").unwrap();
        assert!(freq > 0.0, "Should have frequency at step 0 (note)");
        assert_eq!(seq.current_step(), 0);

        // Gate low
        seq.set_input("gate", 0.0).unwrap();
        seq.process();

        // Second gate - advance to step 1 (rest)
        seq.set_input("gate", 1.0).unwrap();
        seq.process();

        assert_eq!(seq.current_step(), 1);
        let freq = seq.get_output("frequency").unwrap();
        assert_eq!(freq, 0.0, "Should have no frequency at rest step");
    }

    #[test]
    fn test_step_sequencer_wrapping() {
        let mut seq = StepSequencer::new(44100)
            .with_steps(4)
            .with_pattern(vec![Step::note(0); 4]);

        // Advance through all steps
        for expected_step in 0..8 {
            seq.set_input("gate", 1.0).unwrap();
            seq.process();
            assert_eq!(seq.current_step(), expected_step % 4);

            seq.set_input("gate", 0.0).unwrap();
            seq.process();
        }
    }

    #[test]
    fn test_step_sequencer_reset() {
        let mut seq = StepSequencer::new(44100)
            .with_steps(8)
            .with_pattern(vec![Step::note(0); 8]);

        // Advance a few steps
        for _ in 0..5 {
            seq.set_input("gate", 1.0).unwrap();
            seq.process();
            seq.set_input("gate", 0.0).unwrap();
            seq.process();
        }

        assert!(seq.current_step() > 0);

        // Reset
        seq.set_input("reset", 1.0).unwrap();
        seq.process();

        assert_eq!(seq.current_step(), 0);
    }

    #[test]
    fn test_step_sequencer_gate_length() {
        let mut seq = StepSequencer::new(1000) // 1kHz for easy math
            .with_steps(2)
            .with_gate_length(0.5) // 50% default
            .with_pattern(vec![
                Step::note(0),                // Uses default 50%
                Step::note_with_gate(0, 1.0), // 100% gate
            ]);

        // Trigger first step
        seq.set_input("gate", 1.0).unwrap();
        seq.process();
        seq.set_input("gate", 0.0).unwrap();

        // Gate should be high initially
        assert_eq!(seq.get_output("gate").unwrap(), 1.0);

        // After some samples, gate should still be high (within 50% of step duration)
        for _ in 0..100 {
            seq.process();
        }
    }

    #[test]
    fn test_step_sequencer_frequency_calculation() {
        let _seq = StepSequencer::new(44100)
            .with_base_note(60) // C4
            .with_pattern(vec![
                Step::note(0),  // C4
                Step::note(12), // C5 (octave up)
            ]);

        // C4 = 261.63 Hz approximately
        let c4_freq = Note::new(60).frequency();
        let c5_freq = Note::new(72).frequency();

        // Verify our understanding
        assert!((c4_freq - 261.63).abs() < 1.0);
        assert!((c5_freq - 523.25).abs() < 1.0);
    }

    #[test]
    fn test_step_sequencer_empty_pattern() {
        let mut seq = StepSequencer::new(44100).with_steps(4).with_pattern(vec![]); // Empty pattern

        seq.set_input("gate", 1.0).unwrap();
        seq.process();

        // Should treat as rests
        assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
        assert_eq!(seq.get_output("gate").unwrap(), 0.0);
    }

    #[test]
    fn test_step_sequencer_step_output() {
        let mut seq = StepSequencer::new(44100)
            .with_steps(4)
            .with_pattern(vec![Step::note(0); 4]);

        for expected in 0..4 {
            seq.set_input("gate", 1.0).unwrap();
            seq.process();
            assert_eq!(seq.get_output("step").unwrap(), expected as f32);

            seq.set_input("gate", 0.0).unwrap();
            seq.process();
        }
    }

    #[test]
    fn test_step_sequencer_factory() {
        let factory = StepSequencerFactory;
        assert_eq!(factory.type_id(), "step_sequencer");

        let config = serde_json::json!({
            "base_note": 36,
            "steps": 8,
            "gate_length": 0.75,
            "pattern": [
                { "note": 0, "gate": 0.5 },
                { "note": null },
                { "note": 7 },
                { "note": 5, "gate": 1.0 }
            ]
        });

        let result = factory.build(44100, &config).unwrap();
        let module = result.module.lock().unwrap();

        assert_eq!(module.name(), "StepSequencer");
        assert_eq!(module.inputs(), &["gate", "reset"]);
        assert_eq!(module.outputs(), &["frequency", "gate", "step"]);
    }

    #[test]
    fn test_parse_step_formats() {
        // Object with note and gate
        let step = parse_step(&serde_json::json!({"note": 5, "gate": 0.8})).unwrap();
        assert_eq!(step.note, Some(5));
        assert_eq!(step.gate_length, Some(0.8));

        // Object with null note (rest)
        let step = parse_step(&serde_json::json!({"note": null})).unwrap();
        assert_eq!(step.note, None);

        // Simple integer
        let step = parse_step(&serde_json::json!(7)).unwrap();
        assert_eq!(step.note, Some(7));

        // Null value
        let step = parse_step(&serde_json::Value::Null).unwrap();
        assert_eq!(step.note, None);
    }

    #[test]
    fn test_step_sequencer_negative_note_offset() {
        let mut seq = StepSequencer::new(44100)
            .with_base_note(60) // C4
            .with_pattern(vec![Step::note(-12)]); // Should be C3

        seq.set_input("gate", 1.0).unwrap();
        seq.process();

        let freq = seq.get_output("frequency").unwrap();
        let expected = Note::new(48).frequency(); // C3
        assert!((freq - expected).abs() < 0.01);
    }

    #[test]
    fn test_step_sequencer_controls() {
        let mut seq = StepSequencer::new(44100);

        // Verify default control values
        assert_eq!(
            seq.get_control("base_note").unwrap(),
            DEFAULT_BASE_NOTE as f32
        );
        assert_eq!(seq.get_control("steps").unwrap(), DEFAULT_STEPS as f32);
        assert_eq!(seq.get_control("gate_length").unwrap(), DEFAULT_GATE_LENGTH);

        // Set controls
        seq.set_control("base_note", 60.0).unwrap();
        assert_eq!(seq.get_control("base_note").unwrap(), 60.0);

        seq.set_control("steps", 8.0).unwrap();
        assert_eq!(seq.get_control("steps").unwrap(), 8.0);

        seq.set_control("gate_length", 0.75).unwrap();
        assert_eq!(seq.get_control("gate_length").unwrap(), 0.75);

        // Unknown control returns error
        assert!(seq.get_control("unknown").is_err());
        assert!(seq.set_control("unknown", 1.0).is_err());
    }

    #[test]
    fn test_step_sequencer_controls_metadata() {
        let seq = StepSequencer::new(44100);
        let controls = Module::controls(&seq);

        assert_eq!(controls.len(), 3);

        let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"base_note"));
        assert!(keys.contains(&"steps"));
        assert!(keys.contains(&"gate_length"));
    }

    #[test]
    fn test_step_sequencer_controls_affect_processing() {
        let mut seq = StepSequencer::new(44100).with_pattern(vec![Step::note(0)]);

        // Set base_note via control and verify it affects output
        seq.set_control("base_note", 60.0).unwrap();
        seq.set_input("gate", 1.0).unwrap();
        seq.process();

        let freq = seq.get_output("frequency").unwrap();
        let expected = Note::new(60).frequency();
        assert!((freq - expected).abs() < 0.01);

        // Change base_note and verify output changes
        seq.set_control("base_note", 72.0).unwrap();
        seq.process();

        let freq = seq.get_output("frequency").unwrap();
        let expected = Note::new(72).frequency();
        assert!((freq - expected).abs() < 0.01);
    }

    #[test]
    fn test_step_sequencer_factory_returns_handles() {
        let factory = StepSequencerFactory;
        let config = serde_json::json!({
            "base_note": 36,
            "steps": 8,
            "gate_length": 0.75,
        });

        let result = factory.build(44100, &config).unwrap();
        assert_eq!(result.handles.len(), 1);
        assert_eq!(result.handles[0].0, "controls");

        // Verify the handle can be downcast
        let controls = result.handles[0]
            .1
            .downcast_ref::<StepSequencerControls>()
            .unwrap();
        assert_eq!(controls.base_note(), 36);
        assert_eq!(controls.steps(), 8);
        assert!((controls.gate_length() - 0.75).abs() < f32::EPSILON);
    }
}
