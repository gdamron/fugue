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

use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::StepSequencerControls;

mod controls;
mod factory;
mod inputs;
mod outputs;
mod parse;
mod step;

pub use factory::StepSequencerFactory;
pub(crate) use parse::{parse_pattern, parse_step};
pub use step::Step;

/// Default number of steps in a pattern.
pub const DEFAULT_STEPS: usize = 16;

/// Default gate length as a ratio of step duration (0.0-1.0).
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;

/// Default base MIDI note (C3).
pub const DEFAULT_BASE_NOTE: u8 = 48;

/// A deterministic step sequencer for pattern playback.
///
/// Plays back a fixed pattern of notes, advancing one step per clock gate.
/// Supports per-step gate lengths and rests.
///
/// # Inputs
///
/// - `gate` - Clock gate input (rising edge advances step)
/// - `reset` - Reset input (rising edge resets to step 0 and re-arms one-shot)
///
/// # Outputs
///
/// - `frequency` - Current note frequency in Hz (0.0 during rest)
/// - `gate` - Output gate signal (high during note, low during rest)
/// - `step` - Current step number (0 to steps-1)
/// - `end` - End-of-sequence gate: 0.0 while playing; latches to 1.0 when a
///   one-shot pattern completes (exactly one rising edge per playthrough) and
///   stays high until `reset` or a switch back to loop mode
///
/// # Playback modes
///
/// The `mode` control selects `loop` (default; the pattern repeats forever)
/// or `one_shot`: the pattern plays once, the final step sounds for its full
/// duration, and on the next clock edge the sequencer falls silent and fires
/// `end`. Further clock edges are ignored until `reset` re-arms it.
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
    /// Frequency for the current active note, retained across held steps.
    active_note: Option<i8>,
    /// One-shot playback has completed; the `end` output latches high and
    /// clock edges are ignored until reset (or a switch back to loop mode).
    finished: bool,
    /// Force the output gate low for exactly this sample: a new note is
    /// starting while the previous step's gate is still sounding (its
    /// duration was over-estimated — the cold start before the second clock
    /// edge, or a sudden accelerando), so the downstream envelope needs an
    /// explicit release edge to retrigger. Cleared when the outputs are set.
    retrigger_dip: bool,

    // Edge detection state
    last_gate_in: f32,
    last_reset_in: f32,

    // Input values
    inputs: inputs::StepSequencerInputs,

    // Cached outputs
    outputs: outputs::StepSequencerOutputs,
}

impl StepSequencer {
    /// Creates a new step sequencer with the given sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            ctrl: StepSequencerControls::new(),
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2, // Default ~120 BPM
            samples_since_gate: 0,
            first_gate_received: false,
            active_note: None,
            finished: false,
            retrigger_dip: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::StepSequencerInputs::new(),
            outputs: outputs::StepSequencerOutputs::new(),
        }
    }

    /// Creates a new step sequencer with the given sample rate and controls.
    pub fn new_with_controls(sample_rate: u32, controls: StepSequencerControls) -> Self {
        Self {
            sample_rate,
            ctrl: controls,
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2,
            samples_since_gate: 0,
            first_gate_received: false,
            active_note: None,
            finished: false,
            retrigger_dip: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::StepSequencerInputs::new(),
            outputs: outputs::StepSequencerOutputs::new(),
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
    pub fn with_pattern(self, pattern: Vec<Step>) -> Self {
        self.ctrl.set_pattern(pattern);
        self
    }

    /// Enables or disables one-shot playback (the `mode` control).
    pub fn with_one_shot(self, one_shot: bool) -> Self {
        self.ctrl.set_one_shot(one_shot);
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
        self.ctrl.set_pattern(pattern);
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
        self.ctrl.pattern().get(index).cloned().unwrap_or_default()
    }

    /// Calculates the frequency for a note offset.
    fn note_frequency(&self, offset: i8) -> f32 {
        let base = self.ctrl.base_note();
        let midi_note = (base as i16 + offset as i16).clamp(0, 127) as u8;
        Note::new(midi_note).frequency()
    }

    fn next_step_is_held(&self) -> bool {
        let next_step = self.current_step + 1;
        next_step < self.ctrl.steps() && self.get_step(next_step).held
    }

    /// Calculates the gate duration in samples for the current step.
    fn calculate_gate_samples(&self, step: &Step) -> u32 {
        let gate_length = if step.held || self.next_step_is_held() {
            1.0
        } else {
            step.gate_length.unwrap_or(self.ctrl.gate_length())
        };

        (self.step_duration_samples as f32 * gate_length) as u32
    }

    fn start_step(&mut self) {
        let step = self.get_step(self.current_step);
        match step.note {
            Some(offset) => {
                // A new note while the previous gate is still sounding (its
                // duration was over-estimated) needs a forced release edge
                // to retrigger the downstream envelope.
                if self.gate_samples_remaining > 0 {
                    self.retrigger_dip = true;
                }
                self.active_note = Some(offset);
                self.gate_samples_remaining = self.calculate_gate_samples(&step);
            }
            None if step.held && self.active_note.is_some() => {
                self.gate_samples_remaining = self.calculate_gate_samples(&step);
            }
            None => {
                self.active_note = None;
                self.gate_samples_remaining = 0;
            }
        }
    }

    /// Advances to the next step.
    fn advance_step(&mut self) {
        self.current_step = (self.current_step + 1) % self.ctrl.steps();
    }

    /// Resets to step 0 and re-arms one-shot playback.
    fn reset(&mut self) {
        self.current_step = 0;
        self.gate_samples_remaining = 0;
        self.active_note = None;
        self.set_finished(false);
    }

    /// Ends one-shot playback: silence the voice and latch the `end` gate.
    fn finish(&mut self) {
        self.set_finished(true);
        self.active_note = None;
        self.gate_samples_remaining = 0;
    }

    /// Updates `finished` and mirrors it into the controls' read-only `ended`
    /// flag (an event-rate store, not per sample).
    fn set_finished(&mut self, finished: bool) {
        self.finished = finished;
        self.ctrl.set_ended(finished);
    }

    /// Processes one sample.
    fn process_sample(&mut self, i: usize, one_shot: bool) {
        // Detect rising edges
        let gate_rising = self.inputs.gate(i) > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset(i) > 0.5 && self.last_reset_in <= 0.5;

        // Handle reset (takes priority)
        if reset_rising {
            self.reset();
        }

        // Leaving one_shot mode while finished re-arms playback.
        if self.finished && !one_shot {
            self.set_finished(false);
        }

        // Handle clock gate. Once a one-shot pattern has finished, clock
        // edges are ignored until reset.
        if gate_rising && !self.finished {
            // Measure step duration from previous gate
            if self.first_gate_received && self.samples_since_gate > 0 {
                self.step_duration_samples = self.samples_since_gate;
            }

            // Advance step on every gate EXCEPT the first one
            // First gate plays step 0, subsequent gates advance
            if self.first_gate_received {
                if one_shot && self.current_step + 1 >= self.ctrl.steps() {
                    // The final step has sounded for its full duration; this
                    // clock edge is the end of the pattern.
                    self.finish();
                } else {
                    self.advance_step();
                }
            }
            self.first_gate_received = true;
            self.samples_since_gate = 0;

            if !self.finished {
                self.start_step();
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
            i,
            self.active_note
                .map(|offset| self.note_frequency(offset))
                .unwrap_or(0.0),
            if self.retrigger_dip {
                // One-sample forced release so the incoming note retriggers.
                0.0
            } else if self.gate_samples_remaining > 0 {
                1.0
            } else {
                0.0
            },
            self.current_step as f32,
            if self.finished { 1.0 } else { 0.0 },
        );
        // The forced release lasts exactly one sample.
        self.retrigger_dip = false;

        // Store for edge detection
        self.last_gate_in = self.inputs.gate(i);
        self.last_reset_in = self.inputs.reset(i);
    }
}

impl Module for StepSequencer {
    fn name(&self) -> &str {
        "StepSequencer"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Mode is control-plane state: read it once per block so the
        // per-sample loop carries no atomic load for it.
        let one_shot = self.ctrl.one_shot();
        for i in 0..frames {
            self.process_sample(i, one_shot);
        }
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

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
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
            ControlMeta::string(
                "mode",
                "Playback mode: loop repeats; one_shot plays once and fires the end gate",
            )
            .with_options(vec!["loop".to_string(), "one_shot".to_string()])
            .with_default("loop"),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "base_note" => Ok(self.ctrl.base_note() as f32),
            "steps" => Ok(self.ctrl.steps() as f32),
            "gate_length" => Ok(self.ctrl.gate_length()),
            // Numeric view of the string `mode` control: 0.0 = loop, 1.0 = one_shot.
            "mode" => Ok(if self.ctrl.one_shot() { 1.0 } else { 0.0 }),
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
            "mode" => {
                self.ctrl.set_one_shot(value > 0.5);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod retrigger_dip_tests {
    use super::*;

    /// FUG-188 regression (mirrors the cell_sequencer test): an over-estimated
    /// first step must not swallow the retrigger of a note on step 1.
    #[test]
    fn first_step_overrun_still_retriggers_next_note() {
        let mut seq = StepSequencer::new(1000)
            .with_steps(4)
            .with_gate_length(0.95)
            .with_pattern(vec![
                Step::note(0),
                Step::note(5),
                Step::rest(),
                Step::rest(),
            ]);

        let mut rising_edges = 0;
        let mut last_gate = 0.0;
        for sample in 0..400 {
            let clock = if sample % 100 < 50 { 1.0 } else { 0.0 };
            seq.set_input("gate", clock).unwrap();
            seq.process(1);
            let gate = seq.get_output("gate").unwrap();
            if gate > 0.5 && last_gate <= 0.5 {
                rising_edges += 1;
            }
            last_gate = gate;
        }

        assert_eq!(rising_edges, 2);
    }
}
