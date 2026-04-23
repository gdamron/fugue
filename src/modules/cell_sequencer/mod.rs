//! Cell sequencer module for pattern-bank playback.
//!
//! The cell sequencer extends the deterministic step sequencer with multiple
//! stored sequences and controls for selecting or advancing between them.

use serde_json::Value;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::traits::ControlMeta;
use crate::Module;

use super::step_sequencer::{parse_pattern, Step};

pub use self::controls::CellSequencerControls;

mod controls;
mod inputs;
mod outputs;

pub const DEFAULT_STEPS: usize = 16;
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;
pub const DEFAULT_BASE_NOTE: u8 = 48;
pub const MAX_SEQUENCES: usize = 64;

pub struct CellSequencer {
    #[allow(dead_code)]
    sample_rate: u32,
    ctrl: CellSequencerControls,
    sequences: Vec<Vec<Step>>,
    last_sequence_bank_version: u64,
    current_sequence: usize,
    pending_sequence: Option<usize>,
    current_step: usize,
    gate_samples_remaining: u32,
    step_duration_samples: u32,
    samples_since_gate: u32,
    first_gate_received: bool,
    last_gate_in: f32,
    last_reset_in: f32,
    last_next_sequence_in: f32,
    last_previous_sequence_in: f32,
    last_control_selected_sequence: usize,
    inputs: inputs::CellSequencerInputs,
    outputs: outputs::CellSequencerOutputs,
    last_processed_sample: u64,
}

impl CellSequencer {
    pub fn new(sample_rate: u32) -> Self {
        Self::new_with_controls(sample_rate, CellSequencerControls::new())
    }

    pub fn new_with_controls(sample_rate: u32, controls: CellSequencerControls) -> Self {
        let sequences = controls.sequences();
        let current_sequence =
            normalize_sequence_index(controls.selected_sequence(), sequences.len());
        Self {
            sample_rate,
            ctrl: controls.clone(),
            sequences,
            last_sequence_bank_version: controls.sequence_bank_version(),
            current_sequence,
            pending_sequence: None,
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2,
            samples_since_gate: 0,
            first_gate_received: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            last_next_sequence_in: 0.0,
            last_previous_sequence_in: 0.0,
            last_control_selected_sequence: current_sequence,
            inputs: inputs::CellSequencerInputs::new(),
            outputs: outputs::CellSequencerOutputs::new(),
            last_processed_sample: 0,
        }
    }

    pub fn with_base_note(self, base_note: u8) -> Self {
        self.ctrl.set_base_note(base_note);
        self
    }

    pub fn with_steps(self, steps: usize) -> Self {
        self.ctrl.set_steps(steps);
        self
    }

    pub fn with_gate_length(self, gate_length: f32) -> Self {
        self.ctrl.set_gate_length(gate_length);
        self
    }

    pub fn with_selected_sequence(self, selected_sequence: usize) -> Self {
        self.ctrl.set_selected_sequence(selected_sequence);
        self
    }

    pub fn with_wait_for_cycle_end(self, wait_for_cycle_end: bool) -> Self {
        self.ctrl.set_wait_for_cycle_end(wait_for_cycle_end);
        self
    }

    pub fn with_sequences(self, sequences: Vec<Vec<Step>>) -> Self {
        self.ctrl.set_sequences(sequences);
        self
    }

    pub fn current_step(&self) -> usize {
        self.current_step
    }

    pub fn current_sequence(&self) -> usize {
        self.current_sequence
    }

    fn sync_sequences_from_controls(&mut self) {
        let version = self.ctrl.sequence_bank_version();
        if version == self.last_sequence_bank_version {
            return;
        }

        self.sequences = self.ctrl.sequences();
        self.last_sequence_bank_version = version;

        if self.sequences.is_empty() {
            self.current_sequence = 0;
            self.pending_sequence = None;
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.last_control_selected_sequence = 0;
            return;
        }

        self.current_sequence =
            normalize_sequence_index(self.current_sequence, self.sequences.len());
        self.pending_sequence = self
            .pending_sequence
            .map(|index| normalize_sequence_index(index, self.sequences.len()));

        let selected =
            normalize_sequence_index(self.ctrl.selected_sequence(), self.sequences.len());
        self.ctrl.set_selected_sequence(selected);
        self.last_control_selected_sequence = selected;
    }

    fn get_step(&self, sequence_index: usize, step_index: usize) -> Step {
        self.sequences
            .get(sequence_index)
            .and_then(|sequence| sequence.get(step_index))
            .cloned()
            .unwrap_or_default()
    }

    fn current_step_value(&self) -> Step {
        self.get_step(self.current_sequence, self.current_step)
    }

    fn calculate_frequency(&self) -> f32 {
        let step = self.current_step_value();
        match step.note {
            Some(offset) => {
                let midi_note = (self.ctrl.base_note() as i16 + offset as i16).clamp(0, 127) as u8;
                Note::new(midi_note).frequency()
            }
            None => 0.0,
        }
    }

    fn calculate_gate_samples(&self) -> u32 {
        let gate_length = self
            .current_step_value()
            .gate_length
            .unwrap_or(self.ctrl.gate_length());
        (self.step_duration_samples as f32 * gate_length) as u32
    }

    fn effective_selected_sequence(&self) -> usize {
        self.inputs.select_sequence(self.ctrl.selected_sequence())
    }

    fn effective_wait_for_cycle_end(&self) -> bool {
        self.inputs
            .wait_for_cycle_end(self.ctrl.wait_for_cycle_end())
    }

    fn apply_sequence_change(&mut self, sequence_index: usize) {
        let sequence_index = normalize_sequence_index(sequence_index, self.sequences.len());
        self.current_sequence = sequence_index;
        self.current_step = 0;
        self.gate_samples_remaining = 0;
        self.pending_sequence = None;
        self.ctrl.set_selected_sequence(sequence_index);
        self.last_control_selected_sequence = sequence_index;
    }

    fn request_sequence_change(&mut self, sequence_index: usize) {
        if self.sequences.is_empty() {
            self.current_sequence = 0;
            self.pending_sequence = None;
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.ctrl.set_selected_sequence(0);
            self.last_control_selected_sequence = 0;
            return;
        }

        let sequence_index = normalize_sequence_index(sequence_index, self.sequences.len());
        self.ctrl.set_selected_sequence(sequence_index);
        self.last_control_selected_sequence = sequence_index;

        if self.effective_wait_for_cycle_end() {
            self.pending_sequence = Some(sequence_index);
        } else {
            self.apply_sequence_change(sequence_index);
        }
    }

    fn advance_sequence_offset(&self, offset: isize) -> usize {
        if self.sequences.is_empty() {
            return 0;
        }

        let len = self.sequences.len() as isize;
        let base = self.ctrl.selected_sequence() as isize;
        (base + offset).rem_euclid(len) as usize
    }

    fn step_count(&self) -> usize {
        self.ctrl.steps()
    }

    fn update_outputs(&mut self) {
        self.outputs.set(
            self.calculate_frequency(),
            if self.gate_samples_remaining > 0 {
                1.0
            } else {
                0.0
            },
            self.current_step as f32,
            self.current_sequence as f32,
        );
    }

    fn process_sample(&mut self) {
        self.sync_sequences_from_controls();

        let selected_sequence =
            normalize_sequence_index(self.effective_selected_sequence(), self.sequences.len());
        if selected_sequence != self.last_control_selected_sequence {
            self.request_sequence_change(selected_sequence);
        }

        let gate_rising = self.inputs.gate() > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset_gate() > 0.5 && self.last_reset_in <= 0.5;
        let next_rising = self.inputs.next_sequence() > 0.5 && self.last_next_sequence_in <= 0.5;
        let previous_rising =
            self.inputs.previous_sequence() > 0.5 && self.last_previous_sequence_in <= 0.5;

        if reset_rising {
            self.current_step = 0;
            self.gate_samples_remaining = 0;
        }

        if next_rising {
            let target = self.advance_sequence_offset(1);
            self.request_sequence_change(target);
        }

        if previous_rising {
            let target = self.advance_sequence_offset(-1);
            self.request_sequence_change(target);
        }

        if gate_rising {
            if self.first_gate_received && self.samples_since_gate > 0 {
                self.step_duration_samples = self.samples_since_gate;
            }

            if self.first_gate_received {
                let next_step = self.current_step + 1;
                if next_step >= self.step_count() {
                    if let Some(sequence_index) = self.pending_sequence.take() {
                        self.current_sequence =
                            normalize_sequence_index(sequence_index, self.sequences.len());
                        self.ctrl.set_selected_sequence(self.current_sequence);
                        self.last_control_selected_sequence = self.current_sequence;
                    }
                    self.current_step = 0;
                } else {
                    self.current_step = next_step;
                }
            }

            self.first_gate_received = true;
            self.samples_since_gate = 0;

            if self.current_step_value().note.is_some() {
                self.gate_samples_remaining = self.calculate_gate_samples();
            } else {
                self.gate_samples_remaining = 0;
            }
        }

        self.samples_since_gate += 1;

        if self.gate_samples_remaining > 0 {
            self.gate_samples_remaining -= 1;
        }

        self.update_outputs();

        self.last_gate_in = self.inputs.gate();
        self.last_reset_in = self.inputs.reset_gate();
        self.last_next_sequence_in = self.inputs.next_sequence();
        self.last_previous_sequence_in = self.inputs.previous_sequence();
    }
}

impl Module for CellSequencer {
    fn name(&self) -> &str {
        "CellSequencer"
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

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(DEFAULT_BASE_NOTE as f32),
            ControlMeta::new("steps", "Number of steps per sequence")
                .with_range(1.0, 64.0)
                .with_default(DEFAULT_STEPS as f32),
            ControlMeta::new("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(DEFAULT_GATE_LENGTH),
            ControlMeta::new("selected_sequence", "Active sequence index")
                .with_range(0.0, MAX_SEQUENCES as f32 - 1.0)
                .with_default(self.ctrl.selected_sequence() as f32),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "base_note" => Ok(self.ctrl.base_note() as f32),
            "steps" => Ok(self.ctrl.steps() as f32),
            "gate_length" => Ok(self.ctrl.gate_length()),
            "selected_sequence" => Ok(self.ctrl.selected_sequence() as f32),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "base_note" => self.ctrl.set_base_note(value as u8),
            "steps" => self.ctrl.set_steps(value as usize),
            "gate_length" => self.ctrl.set_gate_length(value),
            "selected_sequence" => self.ctrl.set_selected_sequence(value.max(0.0) as usize),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}

pub struct CellSequencerFactory;

impl ModuleFactory for CellSequencerFactory {
    fn type_id(&self) -> &'static str {
        "cell_sequencer"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let base_note = config
            .get("base_note")
            .and_then(|value| value.as_u64())
            .map(|value| value as u8)
            .unwrap_or(DEFAULT_BASE_NOTE);
        let steps = config
            .get("steps")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_STEPS);
        let gate_length = config
            .get("gate_length")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(DEFAULT_GATE_LENGTH);
        let selected_sequence = config
            .get("selected_sequence")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(0);
        let wait_for_cycle_end = config
            .get("wait_for_cycle_end")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let sequences = parse_sequence_bank(config.get("sequences"))?;

        let controls = CellSequencerControls::new_with_values(
            base_note,
            steps,
            gate_length,
            selected_sequence,
            wait_for_cycle_end,
            sequences.clone(),
        );
        let module = CellSequencer::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(module)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

fn normalize_sequence_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}

fn parse_sequence_bank(
    value: Option<&Value>,
) -> Result<Vec<Vec<Step>>, Box<dyn std::error::Error>> {
    let Some(array) = value.and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };

    if array.len() > MAX_SEQUENCES {
        return Err(format!(
            "sequence bank may not contain more than {} sequences",
            MAX_SEQUENCES
        )
        .into());
    }

    let mut bank = Vec::with_capacity(array.len());
    for sequence in array {
        let parsed = parse_pattern(Some(sequence))?;
        if parsed.len() > 64 {
            return Err("each sequence may not contain more than 64 steps".into());
        }
        bank.push(parsed);
    }

    Ok(bank)
}

pub(crate) fn parse_sequence_bank_json(value: &str) -> Result<Vec<Vec<Step>>, String> {
    let value: Value = serde_json::from_str(value).map_err(|err| err.to_string())?;
    parse_sequence_bank(Some(&value)).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControlSurface, ControlValue, ModuleRegistry};

    fn pulse(module: &mut CellSequencer, port: &str) {
        module.set_input(port, 1.0).unwrap();
        module.process();
        module.reset_inputs();
        module.set_input(port, 0.0).unwrap();
        module.process();
    }

    fn advance_gate(module: &mut CellSequencer) {
        pulse(module, "gate");
    }

    #[test]
    fn test_cell_sequencer_basic_playback() {
        let mut seq = CellSequencer::new(44_100).with_sequences(vec![
            vec![Step::note(0), Step::rest(), Step::note(7)],
            vec![Step::note(12)],
        ]);

        advance_gate(&mut seq);
        assert!(seq.get_output("frequency").unwrap() > 0.0);
        assert_eq!(seq.get_output("sequence").unwrap(), 0.0);

        advance_gate(&mut seq);
        assert_eq!(seq.get_output("step").unwrap(), 1.0);
        assert_eq!(seq.get_output("frequency").unwrap(), 0.0);
    }

    #[test]
    fn test_cell_sequencer_next_sequence_switches_immediately() {
        let mut seq = CellSequencer::new(44_100)
            .with_steps(3)
            .with_sequences(vec![
                vec![Step::note(0), Step::note(2), Step::note(4)],
                vec![Step::note(12), Step::note(14), Step::note(16)],
            ]);

        advance_gate(&mut seq);
        pulse(&mut seq, "next_sequence");

        assert_eq!(seq.current_sequence(), 1);
        assert_eq!(seq.current_step(), 0);
        assert_eq!(seq.get_output("sequence").unwrap(), 1.0);
        let expected = Note::new(60).frequency();
        assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);

        advance_gate(&mut seq);
        let expected = Note::new(62).frequency();
        assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    }

    #[test]
    fn test_cell_sequencer_waits_for_cycle_end_before_switching() {
        let mut seq = CellSequencer::new(44_100)
            .with_steps(3)
            .with_wait_for_cycle_end(true)
            .with_sequences(vec![
                vec![Step::note(0), Step::note(2), Step::note(4)],
                vec![Step::note(12), Step::note(14), Step::note(16)],
            ]);

        advance_gate(&mut seq);
        advance_gate(&mut seq);
        pulse(&mut seq, "next_sequence");

        assert_eq!(seq.current_sequence(), 0);
        assert_eq!(seq.current_step(), 1);

        advance_gate(&mut seq);
        assert_eq!(seq.current_sequence(), 0);
        assert_eq!(seq.current_step(), 2);

        advance_gate(&mut seq);
        assert_eq!(seq.current_sequence(), 1);
        assert_eq!(seq.current_step(), 0);
        let expected = Note::new(60).frequency();
        assert!((seq.get_output("frequency").unwrap() - expected).abs() < 0.01);
    }

    #[test]
    fn test_cell_sequencer_wait_for_cycle_end_input_overrides_control() {
        let mut seq = CellSequencer::new(44_100)
            .with_steps(2)
            .with_sequences(vec![
                vec![Step::note(0), Step::note(2)],
                vec![Step::note(12)],
            ]);

        advance_gate(&mut seq);
        seq.reset_inputs();
        seq.set_input("wait_for_cycle_end", 1.0).unwrap();
        seq.set_input("next_sequence", 1.0).unwrap();
        seq.process();

        assert_eq!(seq.current_sequence(), 0);
        assert_eq!(seq.pending_sequence, Some(1));

        seq.reset_inputs();
        seq.set_input("gate", 1.0).unwrap();
        seq.process();
        seq.reset_inputs();
        seq.set_input("gate", 0.0).unwrap();
        seq.process();

        assert_eq!(seq.current_sequence(), 0);

        advance_gate(&mut seq);
        assert_eq!(seq.current_sequence(), 1);
    }

    #[test]
    fn test_cell_sequencer_selected_sequence_control_queues_latest_request() {
        let controls = CellSequencerControls::new_with_values(
            DEFAULT_BASE_NOTE,
            2,
            DEFAULT_GATE_LENGTH,
            0,
            true,
            vec![
                vec![Step::note(0), Step::note(2)],
                vec![Step::note(4), Step::note(5)],
                vec![Step::note(7), Step::note(9)],
            ],
        );
        let mut seq = CellSequencer::new_with_controls(44_100, controls.clone());

        advance_gate(&mut seq);
        controls
            .set_control("selected_sequence", ControlValue::Number(1.0))
            .unwrap();
        seq.process();
        controls
            .set_control("selected_sequence", ControlValue::Number(2.0))
            .unwrap();
        seq.process();

        assert_eq!(seq.pending_sequence, Some(2));

        advance_gate(&mut seq);
        advance_gate(&mut seq);
        assert_eq!(seq.current_sequence(), 2);
    }

    #[test]
    fn test_sequences_json_round_trip() {
        let controls = CellSequencerControls::new();
        controls
            .set_control(
                "sequences_json",
                ControlValue::String(
                    r#"[[{"note":0},{"note":null}],[{"note":12,"gate":0.5}]]"#.to_string(),
                ),
            )
            .unwrap();

        let ControlValue::String(value) = controls.get_control("sequences_json").unwrap() else {
            panic!("sequences_json should be a string");
        };
        let parsed: Value = serde_json::from_str(&value).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_cell_sequencer_factory_and_registry() {
        let factory = CellSequencerFactory;
        let result = factory
            .build(
                44_100,
                &serde_json::json!({
                    "steps": 4,
                    "selected_sequence": 1,
                    "wait_for_cycle_end": true,
                    "sequences": [
                        [{ "note": 0 }],
                        [{ "note": 12 }]
                    ]
                }),
            )
            .unwrap();

        assert!(result.control_surface.is_some());
        assert_eq!(
            result.module.lock().unwrap().outputs(),
            &["frequency", "gate", "step", "sequence"]
        );

        let registry = ModuleRegistry::default();
        assert!(registry.has_type("cell_sequencer"));
    }
}
