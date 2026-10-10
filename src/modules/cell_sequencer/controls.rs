//! Thread-safe controls for the CellSequencer module.
//!
//! Hot-path scalar fields live in atomics so the audio thread can read them
//! lock-free at sample rate. The sequence bank itself sits behind a separate
//! `Mutex` because it's a `Vec<Vec<Step>>`; the audio thread only acquires it
//! when the bank's atomic version counter changes (i.e., after `set_sequences`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::traits::{check_listed_control, read_only, ControlSurfaceMap};
use crate::{ControlMeta, ControlSurface, ControlValue};

use super::{
    parse_sequence_bank_json, Step, DEFAULT_GATE_LENGTH, DEFAULT_GRACE_DURATION,
    DEFAULT_GRACE_VELOCITY, DEFAULT_ROOT_NOTE, DEFAULT_STEPS, MAX_GRACE_DURATION, MAX_SEQUENCES,
    MAX_STEPS, MIN_GRACE_DURATION,
};

#[derive(Clone)]
pub struct CellSequencerControls {
    pub(crate) shared: Arc<CellSequencerShared>,
}

pub(crate) struct CellSequencerShared {
    pub(crate) root_note: AtomicU8,
    pub(crate) step_count: AtomicUsize,
    pub(crate) gate_length: AtomicF32,
    pub(crate) selected_cell: AtomicUsize,
    /// When true, the effective cycle length follows the selected cell's own
    /// length instead of the manual `step_count` control, so switching to a
    /// cell of a different length needs no accompanying `step_count` write
    /// (FUG-239 #10). Default false preserves the manual-`step_count`
    /// behavior.
    pub(crate) follow_cell_length: AtomicBool,
    pub(crate) wait_for_cycle_end: AtomicBool,
    pub(crate) sequence_bank_version: AtomicU64,
    pub(crate) loop_count: AtomicU32,
    pub(crate) cell: AtomicUsize,
    pub(crate) next_cell_request_count: AtomicU64,
    /// One-shot playback flag (the `mode` control: loop | one_shot). The
    /// audio thread reads it at sample rate.
    pub(crate) one_shot: AtomicBool,
    /// Duration of a single grace note in seconds. Seconds, not a
    /// step fraction: acciaccaturas are "as fast as possible" and roughly
    /// tempo-independent. Read once per block by the audio thread.
    pub(crate) grace_duration: AtomicF32,
    /// Velocity scale applied to grace notes relative to the decorated
    /// step's velocity.
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
            DEFAULT_ROOT_NOTE,
            DEFAULT_STEPS,
            DEFAULT_GATE_LENGTH,
            0,
            false,
            Vec::new(),
        )
    }

    pub fn new_with_values(
        root_note: u8,
        step_count: usize,
        gate_length: f32,
        selected_cell: usize,
        wait_for_cycle_end: bool,
        sequences: Vec<Vec<Step>>,
    ) -> Self {
        let selected_cell = clamp_sequence_index(selected_cell, sequences.len());
        Self {
            shared: Arc::new(CellSequencerShared {
                root_note: AtomicU8::new(root_note.min(127)),
                step_count: AtomicUsize::new(step_count.clamp(1, MAX_STEPS)),
                gate_length: AtomicF32::new(gate_length.clamp(0.0, 1.0)),
                selected_cell: AtomicUsize::new(selected_cell),
                follow_cell_length: AtomicBool::new(false),
                wait_for_cycle_end: AtomicBool::new(wait_for_cycle_end),
                sequence_bank_version: AtomicU64::new(0),
                loop_count: AtomicU32::new(0),
                cell: AtomicUsize::new(selected_cell),
                next_cell_request_count: AtomicU64::new(0),
                one_shot: AtomicBool::new(false),
                grace_duration: AtomicF32::new(DEFAULT_GRACE_DURATION),
                grace_velocity: AtomicF32::new(DEFAULT_GRACE_VELOCITY),
                grace_on_beat: AtomicBool::new(false),
                ended: AtomicBool::new(false),
                sequences: Mutex::new(sequences),
            }),
        }
    }

    pub fn root_note(&self) -> u8 {
        self.shared.root_note.load(Ordering::Relaxed)
    }

    pub fn set_root_note(&self, note: u8) {
        self.shared
            .root_note
            .store(note.min(127), Ordering::Relaxed);
    }

    pub fn step_count(&self) -> usize {
        self.shared.step_count.load(Ordering::Relaxed)
    }

    pub fn set_step_count(&self, step_count: usize) {
        self.shared
            .step_count
            .store(step_count.clamp(1, MAX_STEPS), Ordering::Relaxed);
    }

    pub fn gate_length(&self) -> f32 {
        self.shared.gate_length.load()
    }

    pub fn set_gate_length(&self, length: f32) {
        self.shared.gate_length.store(length.clamp(0.0, 1.0));
    }

    /// The cell the `select_cell` control names.
    pub fn selected_cell(&self) -> usize {
        self.shared.selected_cell.load(Ordering::Relaxed)
    }

    pub fn set_selected_cell(&self, selected_cell: usize) {
        let len = self.shared.sequences.lock().unwrap().len();
        self.shared
            .selected_cell
            .store(clamp_sequence_index(selected_cell, len), Ordering::Relaxed);
    }

    /// Whether the cycle length follows the selected cell's own length rather
    /// than the manual `step_count` control (FUG-239 #10).
    pub fn follow_cell_length(&self) -> bool {
        self.shared.follow_cell_length.load(Ordering::Relaxed)
    }

    pub fn set_follow_cell_length(&self, follow_cell_length: bool) {
        self.shared
            .follow_cell_length
            .store(follow_cell_length, Ordering::Relaxed);
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
        self.set_one_shot(mode_is_one_shot(mode)?);
        Ok(())
    }

    /// Duration of a single grace note in seconds.
    pub fn grace_duration(&self) -> f32 {
        self.shared.grace_duration.load()
    }

    pub fn set_grace_duration(&self, seconds: f32) {
        self.shared
            .grace_duration
            .store(seconds.clamp(MIN_GRACE_DURATION, MAX_GRACE_DURATION));
    }

    /// Velocity scale applied to grace notes (relative to the decorated
    /// step's velocity).
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
        self.set_grace_on_beat(grace_is_on_beat(placement)?);
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

    pub fn cells(&self) -> Vec<Vec<Step>> {
        self.shared.sequences.lock().unwrap().clone()
    }

    pub fn set_cells(&self, cells: Vec<Vec<Step>>) {
        let mut bank = self.shared.sequences.lock().unwrap();
        *bank = cells;
        let len = bank.len();
        let selected = self.shared.selected_cell.load(Ordering::Relaxed);
        self.shared
            .selected_cell
            .store(clamp_sequence_index(selected, len), Ordering::Relaxed);
        self.shared
            .sequence_bank_version
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn cells_json(&self) -> String {
        serde_json::to_string(&*self.shared.sequences.lock().unwrap())
            .unwrap_or_else(|_| "[]".to_string())
    }

    pub fn set_cells_json(&self, value: &str) -> Result<(), String> {
        let cells = parse_sequence_bank_json(value)?;
        self.set_cells(cells);
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

    pub fn cell(&self) -> usize {
        self.shared.cell.load(Ordering::Relaxed)
    }

    pub(crate) fn set_cell(&self, value: usize) {
        self.shared.cell.store(value, Ordering::Relaxed);
    }

    pub fn cell_count(&self) -> usize {
        self.shared.sequences.lock().unwrap().len()
    }

    pub fn next_cell_request_count(&self) -> u64 {
        self.shared.next_cell_request_count.load(Ordering::Relaxed)
    }

    pub fn request_next_cell(&self) {
        self.shared
            .next_cell_request_count
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
            ControlMeta::number("root_note", "Root MIDI note")
                .with_range(0.0, 127.0)
                .with_default(self.root_note() as f32),
            ControlMeta::number("step_count", "Number of steps per cell")
                .with_range(1.0, MAX_STEPS as f32)
                .with_default(self.step_count() as f32),
            ControlMeta::boolean(
                "follow_cell_length",
                "Wrap each cycle at the selected cell's own length instead of steps",
                self.follow_cell_length(),
            ),
            ControlMeta::number("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(self.gate_length()),
            ControlMeta::number("select_cell", "Active cell index")
                .with_range(0.0, MAX_SEQUENCES as f32 - 1.0)
                .with_default(self.selected_cell() as f32),
            ControlMeta::boolean(
                "wait_for_cycle_end",
                "Defer cell changes until the current cycle ends",
                self.wait_for_cycle_end(),
            ),
            ControlMeta::string("cells", "Cells as JSON")
                .with_default(self.cells_json()),
            ControlMeta::string(
                "mode",
                "Playback mode: loop repeats the active cell; one_shot plays the bank through once and fires the end gate",
            )
            .with_options(vec!["loop".to_string(), "one_shot".to_string()])
            .with_default(self.mode()),
            ControlMeta::number("loop_count", "Completed loops of the active cell")
                .with_default(self.loop_count() as f32),
            ControlMeta::number("cell", "Currently playing cell index")
                .with_default(self.cell() as f32),
            ControlMeta::number("cell_count", "Total number of cells in the bank")
                .with_default(self.cell_count() as f32),
            ControlMeta::number(
                "next_cell",
                "Trigger: rising edge advances to the next cell",
            )
            .with_default(0.0),
            ControlMeta::number("grace_duration", "Duration of a single grace note in seconds")
                .with_range(MIN_GRACE_DURATION, MAX_GRACE_DURATION)
                .with_default(self.grace_duration()),
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
            "root_note" => Ok((self.root_note() as f32).into()),
            "step_count" => Ok((self.step_count() as f32).into()),
            "follow_cell_length" => Ok(self.follow_cell_length().into()),
            "gate_length" => Ok(self.gate_length().into()),
            "select_cell" => Ok((self.selected_cell() as f32).into()),
            "wait_for_cycle_end" => Ok(self.wait_for_cycle_end().into()),
            "cells" => Ok(self.cells_json().into()),
            "mode" => Ok(self.mode().into()),
            "grace_duration" => Ok(self.grace_duration().into()),
            "grace_velocity" => Ok(self.grace_velocity().into()),
            "grace_placement" => Ok(self.grace_placement().into()),
            "ended" => Ok(self.ended().into()),
            "loop_count" => Ok((self.loop_count() as f32).into()),
            "cell" => Ok((self.cell() as f32).into()),
            "cell_count" => Ok((self.cell_count() as f32).into()),
            "next_cell" => Ok(0.0_f32.into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "root_note" => self.set_root_note(value.as_number()? as u8),
            "step_count" => self.set_step_count(value.as_number()? as usize),
            "follow_cell_length" => self.set_follow_cell_length(value.as_bool()?),
            "gate_length" => self.set_gate_length(value.as_number()?),
            "select_cell" => self.set_selected_cell(value.as_number()?.max(0.0) as usize),
            "wait_for_cycle_end" => self.set_wait_for_cycle_end(value.as_bool()?),
            "cells" => self.set_cells_json(value.as_string()?)?,
            "mode" => self.set_mode(value.as_string()?)?,
            "grace_duration" => self.set_grace_duration(value.as_number()?),
            "grace_velocity" => self.set_grace_velocity(value.as_number()?),
            "grace_placement" => self.set_grace_placement(value.as_string()?)?,
            "next_cell" => {
                if value.as_number()? > 0.5 {
                    self.request_next_cell();
                }
            }
            "loop_count" | "cell" | "cell_count" | "ended" => return read_only(key),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        match key {
            "cells" => parse_sequence_bank_json(value.as_string()?).map(drop),
            "mode" => mode_is_one_shot(value.as_string()?).map(drop),
            "grace_placement" => grace_is_on_beat(value.as_string()?).map(drop),
            "loop_count" | "cell" | "cell_count" | "ended" => read_only(key),
            _ => check_listed_control(&self.controls(), key, value),
        }
    }
}

/// Parses a `mode` value: true for `one_shot`, false for `loop`.
fn mode_is_one_shot(mode: &str) -> Result<bool, String> {
    match mode {
        "loop" => Ok(false),
        "one_shot" => Ok(true),
        other => Err(format!(
            "Unknown mode '{}' (expected loop | one_shot)",
            other
        )),
    }
}

/// Parses a `grace_placement` value: true for `on_beat`, false for `before`.
fn grace_is_on_beat(placement: &str) -> Result<bool, String> {
    match placement {
        "before" => Ok(false),
        "on_beat" => Ok(true),
        other => Err(format!(
            "Unknown grace_placement '{}' (expected before | on_beat)",
            other
        )),
    }
}

fn clamp_sequence_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}
