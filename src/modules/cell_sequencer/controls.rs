//! Thread-safe controls for the CellSequencer module.

use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

use super::{
    parse_sequence_bank_json, Step, DEFAULT_BASE_NOTE, DEFAULT_GATE_LENGTH, DEFAULT_STEPS,
    MAX_SEQUENCES, MAX_STEPS,
};

#[derive(Clone)]
pub struct CellSequencerControls {
    pub(crate) shared: Arc<Mutex<CellSequencerState>>,
}

#[derive(Clone)]
pub(crate) struct CellSequencerState {
    pub(crate) base_note: u8,
    pub(crate) steps: usize,
    pub(crate) gate_length: f32,
    pub(crate) selected_sequence: usize,
    pub(crate) wait_for_cycle_end: bool,
    pub(crate) sequences: Vec<Vec<Step>>,
    pub(crate) sequence_bank_version: u64,
}

impl CellSequencerControls {
    pub fn new() -> Self {
        Self::new_with_values(
            DEFAULT_BASE_NOTE,
            DEFAULT_STEPS,
            DEFAULT_GATE_LENGTH,
            0,
            false,
            Vec::new(),
        )
    }

    pub fn new_with_values(
        base_note: u8,
        steps: usize,
        gate_length: f32,
        selected_sequence: usize,
        wait_for_cycle_end: bool,
        sequences: Vec<Vec<Step>>,
    ) -> Self {
        let selected_sequence = clamp_sequence_index(selected_sequence, sequences.len());
        Self {
            shared: Arc::new(Mutex::new(CellSequencerState {
                base_note: base_note.min(127),
                steps: steps.clamp(1, MAX_STEPS),
                gate_length: gate_length.clamp(0.0, 1.0),
                selected_sequence,
                wait_for_cycle_end,
                sequences,
                sequence_bank_version: 0,
            })),
        }
    }

    pub fn base_note(&self) -> u8 {
        self.shared.lock().unwrap().base_note
    }

    pub fn set_base_note(&self, note: u8) {
        self.shared.lock().unwrap().base_note = note.min(127);
    }

    pub fn steps(&self) -> usize {
        self.shared.lock().unwrap().steps
    }

    pub fn set_steps(&self, steps: usize) {
        self.shared.lock().unwrap().steps = steps.clamp(1, MAX_STEPS);
    }

    pub fn gate_length(&self) -> f32 {
        self.shared.lock().unwrap().gate_length
    }

    pub fn set_gate_length(&self, length: f32) {
        self.shared.lock().unwrap().gate_length = length.clamp(0.0, 1.0);
    }

    pub fn selected_sequence(&self) -> usize {
        self.shared.lock().unwrap().selected_sequence
    }

    pub fn set_selected_sequence(&self, selected_sequence: usize) {
        let mut shared = self.shared.lock().unwrap();
        shared.selected_sequence = clamp_sequence_index(selected_sequence, shared.sequences.len());
    }

    pub fn wait_for_cycle_end(&self) -> bool {
        self.shared.lock().unwrap().wait_for_cycle_end
    }

    pub fn set_wait_for_cycle_end(&self, wait_for_cycle_end: bool) {
        self.shared.lock().unwrap().wait_for_cycle_end = wait_for_cycle_end;
    }

    pub fn sequences(&self) -> Vec<Vec<Step>> {
        self.shared.lock().unwrap().sequences.clone()
    }

    pub fn set_sequences(&self, sequences: Vec<Vec<Step>>) {
        let mut shared = self.shared.lock().unwrap();
        shared.sequences = sequences;
        shared.selected_sequence =
            clamp_sequence_index(shared.selected_sequence, shared.sequences.len());
        shared.sequence_bank_version = shared.sequence_bank_version.wrapping_add(1);
    }

    pub fn sequences_json(&self) -> String {
        serde_json::to_string(&self.sequences()).unwrap_or_else(|_| "[]".to_string())
    }

    pub fn set_sequences_json(&self, value: &str) -> Result<(), String> {
        let sequences = parse_sequence_bank_json(value)?;
        self.set_sequences(sequences);
        Ok(())
    }

    pub fn sequence_bank_version(&self) -> u64 {
        self.shared.lock().unwrap().sequence_bank_version
    }
}

impl Default for CellSequencerControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlSurface for CellSequencerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(self.base_note() as f32),
            ControlMeta::number("steps", "Number of steps per sequence")
                .with_range(1.0, MAX_STEPS as f32)
                .with_default(self.steps() as f32),
            ControlMeta::number("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(self.gate_length()),
            ControlMeta::number("selected_sequence", "Active sequence index")
                .with_range(0.0, MAX_SEQUENCES as f32 - 1.0)
                .with_default(self.selected_sequence() as f32),
            ControlMeta::boolean(
                "wait_for_cycle_end",
                "Defer sequence changes until the current cycle ends",
                self.wait_for_cycle_end(),
            ),
            ControlMeta::string("sequences_json", "Sequence bank as JSON")
                .with_default(self.sequences_json()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "base_note" => Ok((self.base_note() as f32).into()),
            "steps" => Ok((self.steps() as f32).into()),
            "gate_length" => Ok(self.gate_length().into()),
            "selected_sequence" => Ok((self.selected_sequence() as f32).into()),
            "wait_for_cycle_end" => Ok(self.wait_for_cycle_end().into()),
            "sequences_json" => Ok(self.sequences_json().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "base_note" => self.set_base_note(value.as_number()? as u8),
            "steps" => self.set_steps(value.as_number()? as usize),
            "gate_length" => self.set_gate_length(value.as_number()?),
            "selected_sequence" => self.set_selected_sequence(value.as_number()?.max(0.0) as usize),
            "wait_for_cycle_end" => self.set_wait_for_cycle_end(value.as_bool()?),
            "sequences_json" => self.set_sequences_json(value.as_string()?)?,
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}

fn clamp_sequence_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}
