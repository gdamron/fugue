//! Thread-safe controls for the StepSequencer module.

use std::sync::{Arc, Mutex};

use super::{DEFAULT_BASE_NOTE, DEFAULT_GATE_LENGTH, DEFAULT_STEPS};

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
}

impl StepSequencerControls {
    /// Creates new step sequencer controls with default values.
    pub fn new() -> Self {
        Self {
            base_note: Arc::new(Mutex::new(DEFAULT_BASE_NOTE)),
            steps: Arc::new(Mutex::new(DEFAULT_STEPS)),
            gate_length: Arc::new(Mutex::new(DEFAULT_GATE_LENGTH)),
        }
    }

    /// Creates new step sequencer controls with specified values.
    pub fn new_with_values(base_note: u8, steps: usize, gate_length: f32) -> Self {
        Self {
            base_note: Arc::new(Mutex::new(base_note.min(127))),
            steps: Arc::new(Mutex::new(steps.clamp(1, 64))),
            gate_length: Arc::new(Mutex::new(gate_length.clamp(0.0, 1.0))),
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
}

impl Default for StepSequencerControls {
    fn default() -> Self {
        Self::new()
    }
}
