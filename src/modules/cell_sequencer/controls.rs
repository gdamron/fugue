//! Thread-safe controls for the CellSequencer module.
//!
//! Hot-path scalar fields live in atomics so the audio thread can read them
//! lock-free at sample rate. The sequence bank itself sits behind a separate
//! `Mutex` because it's a `Vec<Vec<Step>>`; the audio thread only acquires it
//! when the bank's atomic version counter changes (i.e., after `set_sequences`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

use super::{
    parse_sequence_bank_json, Step, DEFAULT_BASE_NOTE, DEFAULT_GATE_LENGTH,
    DEFAULT_GRACE_DURATION_MS, DEFAULT_GRACE_VELOCITY, DEFAULT_STEPS, MAX_GRACE_DURATION_MS,
    MAX_SEQUENCES, MAX_STEPS, MIN_GRACE_DURATION_MS,
};

#[derive(Clone)]
pub struct CellSequencerControls {
    pub(crate) shared: Arc<CellSequencerShared>,
}

pub(crate) struct CellSequencerShared {
    pub(crate) base_note: AtomicU8,
    pub(crate) steps: AtomicUsize,
    pub(crate) gate_length: AtomicF32,
    pub(crate) selected_sequence: AtomicUsize,
    pub(crate) wait_for_cycle_end: AtomicBool,
    pub(crate) sequence_bank_version: AtomicU64,
    pub(crate) loop_count: AtomicU32,
    pub(crate) current_cell: AtomicUsize,
    pub(crate) advance_request_count: AtomicU64,
    /// One-shot playback flag (the `mode` control: loop | one_shot). The
    /// audio thread reads it at sample rate.
    pub(crate) one_shot: AtomicBool,
    /// Duration of a single grace note in milliseconds. Milliseconds, not a
    /// step fraction: acciaccaturas are "as fast as possible" and roughly
    /// tempo-independent. Read once per block by the audio thread.
    pub(crate) grace_duration_ms: AtomicF32,
    /// Velocity scale applied to grace notes relative to the decorated
    /// step's amplitude.
    pub(crate) grace_velocity: AtomicF32,
    /// Grace placement (the `grace_placement` control): `false` = before the
    /// beat (steal the previous step's tail; the principal stays on the
    /// grid), `true` = on the beat (the chain starts at the step edge and
    /// delays the principal).
    pub(crate) grace_on_beat: AtomicBool,
    /// Written by the audio thread when a one-shot bank playthrough
    /// completes (cleared on re-arm). Exposed as the read-only `ended`
    /// control so live surfaces can observe the end without graph access.
    pub(crate) ended: AtomicBool,
    pub(crate) sequences: Mutex<Vec<Vec<Step>>>,
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
            shared: Arc::new(CellSequencerShared {
                base_note: AtomicU8::new(base_note.min(127)),
                steps: AtomicUsize::new(steps.clamp(1, MAX_STEPS)),
                gate_length: AtomicF32::new(gate_length.clamp(0.0, 1.0)),
                selected_sequence: AtomicUsize::new(selected_sequence),
                wait_for_cycle_end: AtomicBool::new(wait_for_cycle_end),
                sequence_bank_version: AtomicU64::new(0),
                loop_count: AtomicU32::new(0),
                current_cell: AtomicUsize::new(selected_sequence),
                advance_request_count: AtomicU64::new(0),
                one_shot: AtomicBool::new(false),
                grace_duration_ms: AtomicF32::new(DEFAULT_GRACE_DURATION_MS),
                grace_velocity: AtomicF32::new(DEFAULT_GRACE_VELOCITY),
                grace_on_beat: AtomicBool::new(false),
                ended: AtomicBool::new(false),
                sequences: Mutex::new(sequences),
            }),
        }
    }

    pub fn base_note(&self) -> u8 {
        self.shared.base_note.load(Ordering::Relaxed)
    }

    pub fn set_base_note(&self, note: u8) {
        self.shared
            .base_note
            .store(note.min(127), Ordering::Relaxed);
    }

    pub fn steps(&self) -> usize {
        self.shared.steps.load(Ordering::Relaxed)
    }

    pub fn set_steps(&self, steps: usize) {
        self.shared
            .steps
            .store(steps.clamp(1, MAX_STEPS), Ordering::Relaxed);
    }

    pub fn gate_length(&self) -> f32 {
        self.shared.gate_length.load()
    }

    pub fn set_gate_length(&self, length: f32) {
        self.shared.gate_length.store(length.clamp(0.0, 1.0));
    }

    pub fn selected_sequence(&self) -> usize {
        self.shared.selected_sequence.load(Ordering::Relaxed)
    }

    pub fn set_selected_sequence(&self, selected_sequence: usize) {
        let len = self.shared.sequences.lock().unwrap().len();
        self.shared.selected_sequence.store(
            clamp_sequence_index(selected_sequence, len),
            Ordering::Relaxed,
        );
    }

    pub fn wait_for_cycle_end(&self) -> bool {
        self.shared.wait_for_cycle_end.load(Ordering::Relaxed)
    }

    pub fn set_wait_for_cycle_end(&self, wait_for_cycle_end: bool) {
        self.shared
            .wait_for_cycle_end
            .store(wait_for_cycle_end, Ordering::Relaxed);
    }

    /// Returns whether one-shot playback is enabled.
    pub fn one_shot(&self) -> bool {
        self.shared.one_shot.load(Ordering::Relaxed)
    }

    /// Enables or disables one-shot playback.
    pub fn set_one_shot(&self, one_shot: bool) {
        self.shared.one_shot.store(one_shot, Ordering::Relaxed);
    }

    /// Gets the playback mode as its control string (`loop` or `one_shot`).
    pub fn mode(&self) -> &'static str {
        if self.one_shot() {
            "one_shot"
        } else {
            "loop"
        }
    }

    /// Sets the playback mode from its control string.
    pub fn set_mode(&self, mode: &str) -> Result<(), String> {
        match mode {
            "loop" => self.set_one_shot(false),
            "one_shot" => self.set_one_shot(true),
            other => {
                return Err(format!(
                    "Unknown mode '{}' (expected loop | one_shot)",
                    other
                ))
            }
        }
        Ok(())
    }

    /// Duration of a single grace note in milliseconds.
    pub fn grace_duration_ms(&self) -> f32 {
        self.shared.grace_duration_ms.load()
    }

    pub fn set_grace_duration_ms(&self, ms: f32) {
        self.shared
            .grace_duration_ms
            .store(ms.clamp(MIN_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS));
    }

    /// Velocity scale applied to grace notes (relative to the decorated
    /// step's amplitude).
    pub fn grace_velocity(&self) -> f32 {
        self.shared.grace_velocity.load()
    }

    pub fn set_grace_velocity(&self, scale: f32) {
        self.shared.grace_velocity.store(scale.clamp(0.0, 1.0));
    }

    /// Whether grace chains play on the beat (delaying the principal) rather
    /// than before it.
    pub fn grace_on_beat(&self) -> bool {
        self.shared.grace_on_beat.load(Ordering::Relaxed)
    }

    pub fn set_grace_on_beat(&self, on_beat: bool) {
        self.shared.grace_on_beat.store(on_beat, Ordering::Relaxed);
    }

    /// Gets the grace placement as its control string (`before` or `on_beat`).
    pub fn grace_placement(&self) -> &'static str {
        if self.grace_on_beat() {
            "on_beat"
        } else {
            "before"
        }
    }

    /// Sets the grace placement from its control string.
    pub fn set_grace_placement(&self, placement: &str) -> Result<(), String> {
        match placement {
            "before" => self.set_grace_on_beat(false),
            "on_beat" => self.set_grace_on_beat(true),
            other => {
                return Err(format!(
                    "Unknown grace_placement '{}' (expected before | on_beat)",
                    other
                ))
            }
        }
        Ok(())
    }

    /// Whether a one-shot bank playthrough has completed (read-only; the
    /// audio thread maintains it).
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    pub(crate) fn set_ended(&self, ended: bool) {
        self.shared.ended.store(ended, Ordering::Relaxed);
    }

    pub fn sequences(&self) -> Vec<Vec<Step>> {
        self.shared.sequences.lock().unwrap().clone()
    }

    pub fn set_sequences(&self, sequences: Vec<Vec<Step>>) {
        let mut bank = self.shared.sequences.lock().unwrap();
        *bank = sequences;
        let len = bank.len();
        let selected = self.shared.selected_sequence.load(Ordering::Relaxed);
        self.shared
            .selected_sequence
            .store(clamp_sequence_index(selected, len), Ordering::Relaxed);
        self.shared
            .sequence_bank_version
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn sequences_json(&self) -> String {
        serde_json::to_string(&*self.shared.sequences.lock().unwrap())
            .unwrap_or_else(|_| "[]".to_string())
    }

    pub fn set_sequences_json(&self, value: &str) -> Result<(), String> {
        let sequences = parse_sequence_bank_json(value)?;
        self.set_sequences(sequences);
        Ok(())
    }

    pub fn sequence_bank_version(&self) -> u64 {
        self.shared.sequence_bank_version.load(Ordering::Relaxed)
    }

    pub fn loop_count(&self) -> u32 {
        self.shared.loop_count.load(Ordering::Relaxed)
    }

    pub(crate) fn set_loop_count(&self, value: u32) {
        self.shared.loop_count.store(value, Ordering::Relaxed);
    }

    pub fn current_cell(&self) -> usize {
        self.shared.current_cell.load(Ordering::Relaxed)
    }

    pub(crate) fn set_current_cell(&self, value: usize) {
        self.shared.current_cell.store(value, Ordering::Relaxed);
    }

    pub fn total_cells(&self) -> usize {
        self.shared.sequences.lock().unwrap().len()
    }

    pub fn advance_request_count(&self) -> u64 {
        self.shared.advance_request_count.load(Ordering::Relaxed)
    }

    pub fn request_advance(&self) {
        self.shared
            .advance_request_count
            .fetch_add(1, Ordering::Relaxed);
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
            ControlMeta::string(
                "mode",
                "Playback mode: loop repeats the active cell; one_shot plays the bank through once and fires the end gate",
            )
            .with_options(vec!["loop".to_string(), "one_shot".to_string()])
            .with_default(self.mode()),
            ControlMeta::number("loop_count", "Completed loops of the active cell")
                .with_default(self.loop_count() as f32),
            ControlMeta::number("current_cell", "Currently playing cell index")
                .with_default(self.current_cell() as f32),
            ControlMeta::number("total_cells", "Total number of cells in the bank")
                .with_default(self.total_cells() as f32),
            ControlMeta::number(
                "advance",
                "Trigger: rising edge advances to the next cell",
            )
            .with_default(0.0),
            ControlMeta::number("grace_duration_ms", "Duration of a single grace note in ms")
                .with_range(MIN_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS)
                .with_default(self.grace_duration_ms()),
            ControlMeta::number(
                "grace_velocity",
                "Velocity scale for grace notes relative to the decorated step",
            )
            .with_range(0.0, 1.0)
            .with_default(self.grace_velocity()),
            ControlMeta::string(
                "grace_placement",
                "Grace placement: before steals the previous step's tail (principal stays on the grid); on_beat starts the chain at the step edge and delays the principal",
            )
            .with_options(vec!["before".to_string(), "on_beat".to_string()])
            .with_default(self.grace_placement()),
            ControlMeta::boolean(
                "ended",
                "Read-only: a one_shot bank playthrough has completed",
                self.ended(),
            ),
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
            "mode" => Ok(self.mode().into()),
            "grace_duration_ms" => Ok(self.grace_duration_ms().into()),
            "grace_velocity" => Ok(self.grace_velocity().into()),
            "grace_placement" => Ok(self.grace_placement().into()),
            "ended" => Ok(self.ended().into()),
            "loop_count" => Ok((self.loop_count() as f32).into()),
            "current_cell" => Ok((self.current_cell() as f32).into()),
            "total_cells" => Ok((self.total_cells() as f32).into()),
            "advance" => Ok(0.0_f32.into()),
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
            "mode" => self.set_mode(value.as_string()?)?,
            "grace_duration_ms" => self.set_grace_duration_ms(value.as_number()?),
            "grace_velocity" => self.set_grace_velocity(value.as_number()?),
            "grace_placement" => self.set_grace_placement(value.as_string()?)?,
            "advance" => {
                if value.as_number()? > 0.5 {
                    self.request_advance();
                }
            }
            "loop_count" | "current_cell" | "total_cells" | "ended" => {
                return Err(format!("Control '{}' is read-only", key));
            }
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
