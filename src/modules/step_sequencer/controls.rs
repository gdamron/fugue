//! Thread-safe controls for the StepSequencer module.

use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

use super::{Step, DEFAULT_BASE_NOTE, DEFAULT_GATE_LENGTH, DEFAULT_STEPS};

/// Thread-safe controls for the StepSequencer module.
///
/// Controls use the uniform f32 get/set API:
/// - `base_note` - Base MIDI note (0-127)
/// - `steps` - Number of steps in pattern (1-64)
/// - `gate_length` - Default gate length ratio (0.0-1.0)
///
/// # Example
///
/// ```rust,ignore
/// let controls: StepSequencerControls = handles.get("step_sequencer.controls").unwrap();
///
/// // Adjust parameters in real-time
/// controls.set_base_note(36);
/// controls.set_steps(8);
/// controls.set_gate_length(0.75);
/// ```
#[derive(Clone)]
pub struct StepSequencerControls {
    pub(crate) base_note: Arc<Mutex<u8>>,
    pub(crate) steps: Arc<Mutex<usize>>,
    pub(crate) gate_length: Arc<Mutex<f32>>,
    pub(crate) pattern: Arc<Mutex<Vec<Step>>>,
}

impl StepSequencerControls {
    /// Creates new step sequencer controls with default values.
    pub fn new() -> Self {
        Self {
            base_note: Arc::new(Mutex::new(DEFAULT_BASE_NOTE)),
            steps: Arc::new(Mutex::new(DEFAULT_STEPS)),
            gate_length: Arc::new(Mutex::new(DEFAULT_GATE_LENGTH)),
            pattern: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Creates new step sequencer controls with specified values.
    pub fn new_with_values(base_note: u8, steps: usize, gate_length: f32) -> Self {
        Self {
            base_note: Arc::new(Mutex::new(base_note.min(127))),
            steps: Arc::new(Mutex::new(steps.clamp(1, 64))),
            gate_length: Arc::new(Mutex::new(gate_length.clamp(0.0, 1.0))),
            pattern: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Gets the base MIDI note.
    pub fn base_note(&self) -> u8 {
        *self.base_note.lock().unwrap()
    }

    /// Sets the base MIDI note (0-127).
    pub fn set_base_note(&self, note: u8) {
        *self.base_note.lock().unwrap() = note.min(127);
    }

    /// Gets the number of steps.
    pub fn steps(&self) -> usize {
        *self.steps.lock().unwrap()
    }

    /// Sets the number of steps (1-64).
    pub fn set_steps(&self, steps: usize) {
        *self.steps.lock().unwrap() = steps.clamp(1, 64);
    }

    /// Gets the default gate length ratio.
    pub fn gate_length(&self) -> f32 {
        *self.gate_length.lock().unwrap()
    }

    /// Sets the default gate length ratio (0.0-1.0).
    pub fn set_gate_length(&self, length: f32) {
        *self.gate_length.lock().unwrap() = length.clamp(0.0, 1.0);
    }

    /// Gets the current pattern.
    pub fn pattern(&self) -> Vec<Step> {
        self.pattern.lock().unwrap().clone()
    }

    /// Sets the current pattern.
    pub fn set_pattern(&self, pattern: Vec<Step>) {
        *self.pattern.lock().unwrap() = pattern;
    }

    /// Gets the current pattern as JSON.
    ///
    /// This is primarily used by orchestration surfaces such as `agent`, MCP,
    /// and scripts. Fugue's generic control value type does not currently have
    /// a JSON variant, so rich pattern data is exposed as a string control.
    pub fn pattern_json(&self) -> String {
        serde_json::to_string(&self.pattern()).unwrap_or_else(|_| "[]".to_string())
    }

    /// Sets the current pattern from JSON.
    ///
    /// The accepted format is the same step array accepted by the module
    /// config, for example `[{"note":0,"gate":0.5},{"note":null}]`.
    pub fn set_pattern_json(&self, value: &str) -> Result<(), String> {
        let pattern: Vec<Step> = serde_json::from_str(value).map_err(|err| err.to_string())?;
        if pattern.len() > 64 {
            return Err("pattern_json may not contain more than 64 steps".to_string());
        }
        self.set_pattern(pattern);
        Ok(())
    }
}

impl Default for StepSequencerControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlSurface for StepSequencerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(self.base_note() as f32),
            ControlMeta::number("steps", "Number of steps in pattern")
                .with_range(1.0, 64.0)
                .with_default(self.steps() as f32),
            ControlMeta::number("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(self.gate_length()),
            ControlMeta::string("pattern_json", "Step pattern as JSON")
                .with_default(self.pattern_json()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "base_note" => Ok((self.base_note() as f32).into()),
            "steps" => Ok((self.steps() as f32).into()),
            "gate_length" => Ok(self.gate_length().into()),
            "pattern_json" => Ok(self.pattern_json().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "base_note" => self.set_base_note(value.as_number()? as u8),
            "steps" => self.set_steps(value.as_number()? as usize),
            "gate_length" => self.set_gate_length(value.as_number()?),
            "pattern_json" => self.set_pattern_json(value.as_string()?)?,
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
