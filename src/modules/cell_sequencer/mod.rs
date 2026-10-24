//! Cell sequencer module for pattern-bank playback.
//!
//! The cell sequencer extends the deterministic step sequencer with multiple
//! stored cells and controls for selecting or advancing between them.
//!
//! # Dynamics
//!
//! The `velocity` output carries the most recently struck step's `velocity`
//! (1.0 when the step has none), updating only at note onsets — holds, rests,
//! and releases keep the struck level so a ringing tail never jumps. Patch it
//! into a voice's level/brightness (e.g. a VCA `cv`) to realize the score's
//! dynamic marks.
//!
//! # Grace notes
//!
//! A step's `grace` chain (see the score module docs) is realized as short
//! separate attacks on the same mono frequency/gate/velocity stream, each
//! followed by a real release gap so downstream envelopes and voice
//! allocators (`divisi`) see distinct rising edges. Realization is a
//! performance decision, deterministic given three controls:
//!
//! - `grace_duration` (default 0.06 s): duration of each grace. The whole
//!   chain is clamped to half the measured step duration, shrinking
//!   proportionally rather than dropping graces.
//! - `grace_velocity` (default 0.8): velocity scale relative to the
//!   decorated step's `velocity`.
//! - `grace_placement` (default `before`): `before` steals the tail of the
//!   previous step so the principal lands on the grid (acciaccatura);
//!   `on_beat` starts the chain at the step edge and delays the principal
//!   (appoggiatura-leaning). With `before`, a decorated step that has no
//!   previous step to steal from (cold start, or step 0 after a cell
//!   boundary — the look-ahead never crosses cells) falls back to on-beat
//!   realization; if the real clock edge arrives while a chain is still
//!   sounding (accelerando), the chain is truncated and the principal wins.
//!
//! # Playback modes
//!
//! The `mode` control selects `loop` (default: the active cell repeats until
//! a command switches cells) or `one_shot`: at each cycle end the sequencer
//! auto-advances to the next cell, playing the bank through once as a single
//! long sequence; after the final cell's last step completes it falls silent
//! and latches the `ended` output gate high (exactly one rising edge). Explicit
//! cell commands (`select_cell`, `next_cell`/`previous_cell`) still work in
//! one_shot and take priority over the auto-advance; `reset` or selecting a
//! cell re-arms a finished sequencer.
//!
//! # Selecting a cell
//!
//! A `select_cell` write is a request: the control reads back the cell
//! last asked for, and the sequencer never writes it. The `cell` control
//! (telemetry) reports the cell playing, which moves as a deferred
//! selection lands at a cycle end or one-shot playback advances. Every
//! selection is acted on as it applies: one naming a cell other than the
//! one the sequencer is headed for (the cell waiting to start, or else the
//! cell playing) switches to it, at once or at the cycle end. A request
//! past the bank's end plays its last cell: it is clamped against the bank
//! as it is acted on, so a selection written with a new bank finds it.

use serde_json::Value;
use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::music::Note;
use crate::payload::{Payload, Retired, Shared};
use crate::traits::ControlMeta;
use crate::Module;

use super::step_sequencer::grace::{clamp_per_grace, release_gap, GracePlayer, GraceVoice};
pub(crate) use super::step_sequencer::grace::{
    DEFAULT_GRACE_DURATION, DEFAULT_GRACE_VELOCITY, MAX_GRACE_DURATION, MIN_GRACE_DURATION,
};
use super::step_sequencer::{parse_pattern, GraceChain, Step, StepError};

pub use self::factory::CellSequencerFactory;
use self::table::{
    Bank, CELL, CELLS, CELL_COUNT, ENDED, FOLLOW_CELL_LENGTH, GATE_LENGTH, GRACE_DURATION,
    GRACE_PLACEMENT, GRACE_VELOCITY, LOOP_COUNT, MODE, NEXT_CELL, ROOT_NOTE, SELECT_CELL,
    STEP_COUNT, TABLE, WAIT_FOR_CYCLE_END,
};

mod apply;
mod factory;
mod inputs;
mod outputs;
mod parse;
mod table;

pub(crate) use parse::parse_sequence_bank_json;
use parse::{normalize_sequence_index, parse_sequence_bank};

pub const DEFAULT_STEPS: usize = 16;
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;
pub const DEFAULT_ROOT_NOTE: u8 = 48;
pub const MAX_SEQUENCES: usize = 64;
/// Upper bound on steps per cell. This sanity-bounds `cells` input (a bank
/// is built on a control thread and handed over whole), not playback cost —
/// playback is O(1) per sample regardless of length. 1024 lets a full
/// through-composed lane live in a single cell.
pub const MAX_STEPS: usize = 1024;

pub struct CellSequencer {
    sample_rate: u32,
    root_note: u8,
    step_count: usize,
    follow_cell_length: bool,
    gate_length: f32,
    wait_for_cycle_end: bool,
    /// One-shot playback (the `mode` control: loop | one_shot).
    one_shot: bool,
    /// Duration of a single grace note in seconds. Seconds, not a step
    /// fraction: acciaccaturas are "as fast as possible" and roughly
    /// tempo-independent.
    grace_duration: f32,
    /// Velocity scale applied to grace notes relative to the decorated
    /// step's velocity.
    grace_velocity: f32,
    /// Grace placement (the `grace_placement` control): `false` = before
    /// the beat, `true` = on the beat.
    grace_on_beat: bool,
    loop_count: u32,
    cells: Arc<ControlCells>,
    /// The bank, kept whole: a written one arrives as a payload, and the one
    /// it replaces is retired off the audio thread.
    sequences: Shared<Bank>,
    /// The cell the sequencer is headed for: the deferred one while a
    /// selection waits for the cycle end, otherwise the one playing. Cell
    /// commands move relative to it, and a selection of it is already met.
    selected: usize,
    /// A `select_cell` request applied since the last sample, acted on at
    /// the next one.
    requested: Option<usize>,
    /// The `next_cell` triggers applied since the last sample: each is an
    /// event of its own, so two at one sample advance two cells.
    next_requested: u32,
    current_sequence: usize,
    pending_sequence: Option<usize>,
    current_step: usize,
    gate_samples_remaining: u32,
    /// When `true`, the output gate stays high regardless of
    /// `gate_samples_remaining`. Set when the active step is part of a held
    /// chain that should sustain across the next clock boundary; cleared as
    /// soon as the next `start_step` runs.
    gate_continuous: bool,
    step_duration_samples: u32,
    samples_since_gate: u32,
    first_gate_received: bool,
    active_note: Option<i8>,
    /// One-shot playback of the bank has completed; `ended` latches high and
    /// clock edges are ignored until reset or an explicit cell selection.
    finished: bool,
    /// Force the output gate low for exactly this sample: a new note is
    /// starting while the previous step's gate is still sounding (its
    /// duration was over-estimated — the cold start before the second clock
    /// edge, or a sudden accelerando), so the downstream envelope needs an
    /// explicit release edge to retrigger. Cleared by `update_outputs`.
    retrigger_dip: bool,
    /// Velocity of the most recently struck note (the step's `velocity`,
    /// 1.0 when unset). Held on the `velocity` output across holds, rests,
    /// and releases so a ringing tail never sees its level jump; it only
    /// changes when a new note starts.
    active_velocity: f32,
    /// Plays a step's grace chain as short attacks ahead of its principal.
    grace_player: GracePlayer,
    /// Countdown to a pre-scheduled (before-the-beat) grace chain for the
    /// upcoming step, in samples from the current step's edge; 0 = none.
    grace_countdown: u32,
    pending_grace: GraceChain,
    pending_grace_velocity: f32,
    pending_grace_per: u32,
    /// The upcoming step's chain actually started sounding off the previous
    /// step's tail (before placement), so `start_step` must not replay it on
    /// the beat. Left false when a pre-scheduled countdown never fired (the
    /// edge came early), which falls back to on-beat realization.
    grace_prescheduled: bool,
    /// Principal note waiting for an on-beat grace chain to finish, with
    /// its velocity, remaining gate samples, and whether it bridges into a
    /// following held step.
    deferred_note: Option<i8>,
    deferred_velocity: f32,
    deferred_gate_samples: u32,
    deferred_gate_continuous: bool,
    /// Block-rate caches of the grace controls (read once per `process`).
    grace_samples_cfg: u32,
    grace_velocity_cfg: f32,
    grace_on_beat_cfg: bool,
    last_gate_in: f32,
    last_reset_in: f32,
    last_next_cell_in: f32,
    last_previous_cell_in: f32,
    cached_frequency: f32,
    cached_frequency_offset: Option<i8>,
    cached_frequency_base: u8,
    inputs: inputs::CellSequencerInputs,
    outputs: outputs::CellSequencerOutputs,
}

impl CellSequencer {
    pub fn new(sample_rate: u32) -> Self {
        let cells = Arc::new(ControlCells::new(table::defaults()));
        Self::with_parts(sample_rate, cells, Shared::new(Vec::new()), 0)
    }

    /// A cell sequencer holding what `cells` hold, applied (so clamped),
    /// playing `bank` from cell `start_cell`.
    fn with_parts(
        sample_rate: u32,
        cells: Arc<ControlCells>,
        bank: Shared<Bank>,
        start_cell: usize,
    ) -> Self {
        let current_sequence = normalize_sequence_index(start_cell, bank.len());
        let mut seq = Self {
            sample_rate,
            root_note: DEFAULT_ROOT_NOTE,
            step_count: DEFAULT_STEPS,
            follow_cell_length: false,
            gate_length: DEFAULT_GATE_LENGTH,
            wait_for_cycle_end: false,
            one_shot: false,
            grace_duration: DEFAULT_GRACE_DURATION,
            grace_velocity: DEFAULT_GRACE_VELOCITY,
            grace_on_beat: false,
            loop_count: 0,
            cells,
            sequences: bank,
            selected: current_sequence,
            requested: None,
            next_requested: 0,
            current_sequence,
            pending_sequence: None,
            current_step: 0,
            gate_samples_remaining: 0,
            gate_continuous: false,
            step_duration_samples: sample_rate / 2,
            samples_since_gate: 0,
            first_gate_received: false,
            active_note: None,
            finished: false,
            retrigger_dip: false,
            active_velocity: 1.0,
            grace_player: GracePlayer::idle(),
            grace_countdown: 0,
            pending_grace: GraceChain::default(),
            pending_grace_velocity: 1.0,
            pending_grace_per: 0,
            grace_prescheduled: false,
            deferred_note: None,
            deferred_velocity: 1.0,
            deferred_gate_samples: 0,
            deferred_gate_continuous: false,
            grace_samples_cfg: 0,
            grace_velocity_cfg: DEFAULT_GRACE_VELOCITY,
            grace_on_beat_cfg: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            last_next_cell_in: 0.0,
            last_previous_cell_in: 0.0,
            cached_frequency: 0.0,
            cached_frequency_offset: None,
            cached_frequency_base: DEFAULT_ROOT_NOTE,
            inputs: inputs::CellSequencerInputs::new(),
            outputs: outputs::CellSequencerOutputs::new(),
        };
        // The parameters the cells hold. The selection is the start cell,
        // not a request, and a trigger or telemetry is never replayed.
        for control in [
            ROOT_NOTE,
            STEP_COUNT,
            FOLLOW_CELL_LENGTH,
            GATE_LENGTH,
            WAIT_FOR_CYCLE_END,
            MODE,
            GRACE_DURATION,
            GRACE_VELOCITY,
            GRACE_PLACEMENT,
        ] {
            if let Some(value) = seq.cells.load(control) {
                let _ = apply_declared(&mut seq, control, value);
            }
        }
        seq.cached_frequency_base = seq.root_note;
        // The start cell reads back as asked for, as a request does.
        let start = RtValue::I32(start_cell.min(MAX_SEQUENCES - 1) as i32);
        seq.cells.publish(SELECT_CELL, start);
        seq.set_cell(current_sequence);
        seq.cells
            .publish(CELL_COUNT, RtValue::I32(seq.sequences.len() as i32));
        seq
    }

    fn set(&mut self, control: ControlIndex, value: RtValue) {
        let _ = apply_declared(self, control, value);
    }

    pub fn with_root_note(mut self, root_note: u8) -> Self {
        self.set(ROOT_NOTE, RtValue::I32(i32::from(root_note)));
        self
    }

    pub fn with_step_count(mut self, step_count: usize) -> Self {
        let step_count = step_count.min(MAX_STEPS) as i32;
        self.set(STEP_COUNT, RtValue::I32(step_count));
        self
    }

    pub fn with_follow_cell_length(mut self, follow_cell_length: bool) -> Self {
        self.set(FOLLOW_CELL_LENGTH, RtValue::Bool(follow_cell_length));
        self
    }

    pub fn with_gate_length(mut self, gate_length: f32) -> Self {
        self.set(GATE_LENGTH, RtValue::F32(gate_length));
        self
    }

    /// Requests `selected_cell`, as a `select_cell` write does: the next
    /// sample switches to it (at the cycle end, when waiting for one).
    pub fn with_selected_cell(mut self, selected_cell: usize) -> Self {
        let cell = selected_cell.min(MAX_SEQUENCES - 1) as i32;
        self.set(SELECT_CELL, RtValue::I32(cell));
        self
    }

    pub fn with_wait_for_cycle_end(mut self, wait_for_cycle_end: bool) -> Self {
        self.set(WAIT_FOR_CYCLE_END, RtValue::Bool(wait_for_cycle_end));
        self
    }

    /// Replaces the bank. Allocates, and frees the one it replaces: never
    /// call it on the audio thread.
    pub fn with_cells(mut self, cells: Vec<Vec<Step>>) -> Self {
        drop(self.adopt_bank(Shared::new(cells)));
        self
    }

    /// Enables or disables one-shot playback (the `mode` control).
    pub fn with_one_shot(mut self, one_shot: bool) -> Self {
        self.set(MODE, RtValue::U32(u32::from(one_shot)));
        self
    }

    /// Plays grace chains on the beat (`true`) or before it.
    pub fn with_grace_on_beat(mut self, on_beat: bool) -> Self {
        self.set(GRACE_PLACEMENT, RtValue::U32(u32::from(on_beat)));
        self
    }

    pub fn current_step(&self) -> usize {
        self.current_step
    }

    pub fn current_cell(&self) -> usize {
        self.current_sequence
    }

    /// Takes `bank` whole, from the next sample, returning the bank it
    /// replaces for retiring. The playing and waiting cells, and the
    /// selection, are clamped into it. Allocation-, free- and lock-free.
    fn adopt_bank(&mut self, bank: Shared<Bank>) -> Shared<Bank> {
        let replaced = std::mem::replace(&mut self.sequences, bank);
        let len = self.sequences.len();
        self.cells.publish(CELL_COUNT, RtValue::I32(len as i32));

        if len == 0 {
            self.current_sequence = 0;
            self.pending_sequence = None;
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.gate_continuous = false;
            self.active_note = None;
            self.clear_grace_state();
            self.selected = 0;
            self.set_cell(0);
            self.set_loop_count(0);
            return replaced;
        }

        self.current_sequence = normalize_sequence_index(self.current_sequence, len);
        self.pending_sequence = self
            .pending_sequence
            .map(|index| normalize_sequence_index(index, len));
        self.selected = normalize_sequence_index(self.selected, len);
        self.set_cell(self.current_sequence);
        replaced
    }

    /// Publishes the cell playing (telemetry).
    fn set_cell(&self, cell: usize) {
        self.cells.publish(CELL, RtValue::I32(cell as i32));
    }

    /// Sets and publishes the completed loops of the playing cell.
    fn set_loop_count(&mut self, loops: u32) {
        self.loop_count = loops;
        let shown = loops.min(i32::MAX as u32) as i32;
        self.cells.publish(LOOP_COUNT, RtValue::I32(shown));
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

    fn note_frequency(&self, offset: i8) -> f32 {
        let midi_note = (self.root_note as i16 + offset as i16).clamp(0, 127) as u8;
        Note::new(midi_note).frequency()
    }

    fn next_step_is_held(&self) -> bool {
        let next_step = self.current_step + 1;
        next_step < self.step_count() && self.get_step(self.current_sequence, next_step).held
    }

    /// Whether the step after the current one starts a new note — i.e. it is a
    /// note step (not held, not a rest). Such a step must retrigger, so the
    /// current step's gate has to release first.
    fn next_step_starts_note(&self) -> bool {
        let next_step = self.current_step + 1;
        next_step < self.step_count()
            && self
                .get_step(self.current_sequence, next_step)
                .note
                .is_some()
    }

    fn gate_samples_for_step(&self, step: &Step, retrigger_follows: bool) -> u32 {
        // Used for non-bridged steps: an end-of-chain held step or an
        // ordinary note step. A held step normally fills its full duration,
        // but when a new note follows it must release first so that note
        // retriggers — relying on the one-sample boundary dip is unreliable at
        // fractional step durations (fast tempos drop the retrigger). Note
        // steps always respect the per-step or default gate_length.
        let gate_length = if step.held {
            if retrigger_follows {
                self.gate_length
            } else {
                1.0
            }
        } else {
            step.gate_length.unwrap_or(self.gate_length)
        };
        (self.step_duration_samples as f32 * gate_length) as u32
    }

    fn start_step(&mut self) {
        // Whether the previous step's gate is still sounding at this edge. A
        // correctly measured step releases before the boundary (see
        // `docs/musical-time.md`), but an over-estimated duration — the cold
        // start before the second clock edge, or a sudden accelerando — can
        // leave it high.
        let gate_still_high = self.gate_continuous
            || self.gate_samples_remaining > 0
            || self.grace_player.is_active();
        let step = self.current_step_value();
        let next_held = self.next_step_is_held();

        // The edge closes out any grace playback in flight: an overrunning
        // chain is truncated (the principal always wins), and a stale
        // pre-schedule whose countdown never fired (the edge came earlier
        // than the measured step duration predicted) is dropped.
        let prescheduled = self.grace_prescheduled;
        self.grace_prescheduled = false;
        self.grace_countdown = 0;
        self.grace_player.cancel();
        // An unsounded deferred principal is musically past; the new step wins.
        self.deferred_note = None;
        self.deferred_gate_continuous = false;

        let voice_active = match step.note {
            Some(offset) => {
                // A new onset needs a rising edge; force the release the
                // previous step failed to provide.
                if gate_still_high {
                    self.retrigger_dip = true;
                }
                let chain_on_beat =
                    !step.grace.is_empty() && (self.grace_on_beat_cfg || !prescheduled);
                if chain_on_beat {
                    // Play the chain from this edge and defer the principal
                    // until it ends: on_beat placement, or the fallback when
                    // no previous step could host a before-the-beat chain
                    // (cold start, or step 0 after a cell boundary — the
                    // look-ahead never crosses cells).
                    let per = clamp_per_grace(
                        &step.grace,
                        self.grace_samples_cfg,
                        self.step_duration_samples / 2,
                    );
                    let velocity = step.velocity.unwrap_or(1.0);
                    self.grace_player
                        .start(step.grace, velocity * self.grace_velocity_cfg, per);
                    self.deferred_note = Some(offset);
                    self.deferred_velocity = velocity;
                    self.deferred_gate_continuous = next_held;
                    let chain_total = GracePlayer::chain_samples(&step.grace, per);
                    self.deferred_gate_samples = self
                        .gate_samples_for_step(&step, self.next_step_starts_note())
                        .saturating_sub(chain_total);
                } else {
                    self.active_note = Some(offset);
                    self.active_velocity = step.velocity.unwrap_or(1.0);
                }
                true
            }
            None if step.held && self.active_note.is_some() => true,
            None => {
                self.active_note = None;
                false
            }
        };

        if !voice_active || self.deferred_note.is_some() {
            // Silent step, or the gate belongs to the grace chain until the
            // deferred principal starts.
            self.gate_continuous = false;
            self.gate_samples_remaining = 0;
        } else if next_held {
            // Bridge the gate across the upcoming clock boundary so the
            // downstream envelope doesn't see a one-sample dip and retrigger.
            self.gate_continuous = true;
            self.gate_samples_remaining = 0;
        } else {
            self.gate_continuous = false;
            let retrigger_follows = self.next_step_starts_note();
            self.gate_samples_remaining = self.gate_samples_for_step(&step, retrigger_follows);
        }

        // Before-the-beat placement: a decorated upcoming step steals this
        // step's tail so its principal lands on the grid. Runs for every
        // step (a rest can host a chain too).
        self.maybe_preschedule_next();
    }

    /// Schedules the next step's grace chain (before-the-beat placement) to
    /// sound at the tail of the current step, ending at the predicted next
    /// edge, and releases this step's gate early enough that the chain's
    /// first onset presents a real rising edge. The look-ahead stays within
    /// the current cell; a decorated step 0 of the following cell falls back
    /// to on-beat realization in `start_step` instead.
    fn maybe_preschedule_next(&mut self) {
        if self.grace_on_beat_cfg {
            return;
        }
        let next_index = self.current_step + 1;
        if next_index >= self.step_count() {
            return;
        }
        let next = self.get_step(self.current_sequence, next_index);
        if next.note.is_none() || next.grace.is_empty() {
            return;
        }

        let per = clamp_per_grace(
            &next.grace,
            self.grace_samples_cfg,
            self.step_duration_samples / 2,
        );
        let total = GracePlayer::chain_samples(&next.grace, per);
        let start_at = self.step_duration_samples.saturating_sub(total);
        if start_at == 0 {
            return;
        }

        self.pending_grace = next.grace;
        self.pending_grace_velocity = next.velocity.unwrap_or(1.0) * self.grace_velocity_cfg;
        self.pending_grace_per = per;
        self.grace_countdown = start_at;

        // The chain has a note step ahead of it, so `next_held` was false
        // and the gate is sample-counted (never a held bridge): cap it so it
        // releases a gap before the chain's first onset.
        let cap = start_at.saturating_sub(release_gap(per));
        self.gate_samples_remaining = self.gate_samples_remaining.min(cap);
        self.deferred_gate_samples = self.deferred_gate_samples.min(cap);
    }

    /// Silences any grace playback and scheduling (reset, cell change, end
    /// of a one-shot bank).
    fn clear_grace_state(&mut self) {
        self.grace_player.cancel();
        self.grace_countdown = 0;
        self.grace_prescheduled = false;
        self.deferred_note = None;
        self.deferred_gate_continuous = false;
    }

    fn prime_current_note(&mut self) {
        let step = self.current_step_value();
        self.active_note = step.note;
    }

    /// Ends one-shot playback: silence the voice and latch the `ended` gate.
    fn finish(&mut self) {
        self.set_finished(true);
        self.active_note = None;
        self.gate_samples_remaining = 0;
        self.gate_continuous = false;
        self.clear_grace_state();
    }

    /// Updates `finished` and publishes it as the read-only `ended`
    /// control (an event-rate store, not per sample).
    fn set_finished(&mut self, finished: bool) {
        self.finished = finished;
        self.cells.publish(ENDED, RtValue::Bool(finished));
    }

    /// The cell a selection asks for at sample `i`, if one does: the
    /// `select_cell` input while connected, which owns the selection, or
    /// else a request applied since the last sample.
    fn selection_at(&mut self, i: usize) -> Option<usize> {
        let requested = self.requested.take();
        self.inputs.select_cell(i).or(requested)
    }

    fn effective_wait_for_cycle_end(&self, i: usize) -> bool {
        self.inputs.wait_for_cycle_end(i, self.wait_for_cycle_end)
    }

    fn apply_sequence_change(&mut self, sequence_index: usize) {
        let sequence_index = normalize_sequence_index(sequence_index, self.sequences.len());
        // An explicit cell selection re-arms a finished one-shot sequencer.
        self.set_finished(false);
        self.current_sequence = sequence_index;
        self.current_step = 0;
        self.gate_samples_remaining = 0;
        self.gate_continuous = false;
        self.active_note = None;
        self.pending_sequence = None;
        self.clear_grace_state();
        self.prime_current_note();
        self.selected = sequence_index;
        self.set_cell(sequence_index);
        self.set_loop_count(0);
    }

    fn request_sequence_change(&mut self, i: usize, sequence_index: usize) {
        if self.sequences.is_empty() {
            self.current_sequence = 0;
            self.pending_sequence = None;
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.gate_continuous = false;
            self.active_note = None;
            self.clear_grace_state();
            self.selected = 0;
            return;
        }

        let sequence_index = normalize_sequence_index(sequence_index, self.sequences.len());
        self.selected = sequence_index;

        if self.effective_wait_for_cycle_end(i) && !self.finished {
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
        let base = self.selected as isize;
        (base + offset).rem_euclid(len) as usize
    }

    fn step_count(&self) -> usize {
        // In follow-cell-length mode the cycle spans the playing cell's own length, so a
        // cell change of a different length needs no accompanying `step_count`
        // write. Reads the bank the sequencer holds, without a lock.
        // An empty or missing cell falls back to the manual `step_count` control so
        // the cycle length is never zero.
        if self.follow_cell_length {
            match self.sequences.get(self.current_sequence) {
                Some(cell) if !cell.is_empty() => return cell.len().min(MAX_STEPS),
                _ => {}
            }
        }
        self.step_count
    }

    fn frequency_for_active_note(&mut self) -> f32 {
        let Some(offset) = self.active_note else {
            self.cached_frequency_offset = None;
            return 0.0;
        };
        let base = self.root_note;
        if self.cached_frequency_offset == Some(offset) && self.cached_frequency_base == base {
            return self.cached_frequency;
        }
        let freq = self.note_frequency(offset);
        self.cached_frequency = freq;
        self.cached_frequency_offset = Some(offset);
        self.cached_frequency_base = base;
        freq
    }

    fn update_outputs(&mut self, i: usize, grace: Option<GraceVoice>) {
        // An active grace chain owns the mono output stream; the principal
        // (and its cached frequency) resume when the chain ends.
        let (frequency, gate_high, velocity) = match grace {
            Some(voice) => (
                self.note_frequency(voice.offset),
                voice.gate,
                voice.velocity,
            ),
            None => (
                self.frequency_for_active_note(),
                self.gate_continuous || self.gate_samples_remaining > 0,
                self.active_velocity,
            ),
        };
        let gate = if self.retrigger_dip {
            // One-sample forced release so the incoming note retriggers.
            self.retrigger_dip = false;
            0.0
        } else if gate_high {
            1.0
        } else {
            0.0
        };
        self.outputs.set(
            i,
            frequency,
            gate,
            velocity,
            self.current_step as f32,
            self.current_sequence as f32,
            if self.finished { 1.0 } else { 0.0 },
        );
    }

    fn process_sample(&mut self, i: usize, one_shot: bool) {
        if let Some(cell) = self.selection_at(i) {
            let cell = normalize_sequence_index(cell, self.sequences.len());
            if cell != self.selected {
                self.request_sequence_change(i, cell);
            }
        }

        let requested_advances = std::mem::take(&mut self.next_requested);

        let gate_rising = self.inputs.clock(i) > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset_gate(i) > 0.5 && self.last_reset_in <= 0.5;
        let next_rising = self.inputs.next_cell(i) > 0.5 && self.last_next_cell_in <= 0.5;
        let advances = requested_advances.saturating_add(next_rising as u32);
        let previous_rising =
            self.inputs.previous_cell(i) > 0.5 && self.last_previous_cell_in <= 0.5;

        if reset_rising {
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.gate_continuous = false;
            self.active_note = None;
            self.clear_grace_state();
            self.set_finished(false);
        }

        if advances > 0 {
            let target = self.advance_sequence_offset(advances as isize);
            self.request_sequence_change(i, target);
        }

        if previous_rising {
            let target = self.advance_sequence_offset(-1);
            self.request_sequence_change(i, target);
        }

        // Leaving one_shot mode while finished re-arms playback.
        if self.finished && !one_shot {
            self.set_finished(false);
        }

        // Once a one-shot bank has finished, clock edges are ignored until
        // reset or an explicit cell selection.
        if gate_rising && !self.finished {
            if self.first_gate_received && self.samples_since_gate > 0 {
                self.step_duration_samples = self.samples_since_gate;
            }

            if self.first_gate_received {
                let next_step = self.current_step + 1;
                if next_step >= self.step_count() {
                    if let Some(sequence_index) = self.pending_sequence.take() {
                        // Explicit (deferred) cell commands take priority
                        // over one_shot auto-advance.
                        self.current_sequence =
                            normalize_sequence_index(sequence_index, self.sequences.len());
                        self.selected = self.current_sequence;
                        self.active_note = None;
                        self.set_cell(self.current_sequence);
                        self.set_loop_count(0);
                        self.current_step = 0;
                    } else if one_shot {
                        if self.current_sequence + 1 < self.sequences.len() {
                            // Play the bank through: fall into the next cell.
                            self.current_sequence += 1;
                            self.selected = self.current_sequence;
                            self.active_note = None;
                            self.set_cell(self.current_sequence);
                            self.set_loop_count(0);
                            self.current_step = 0;
                        } else {
                            // The final cell's last step has sounded for its
                            // full duration: the bank is over.
                            self.finish();
                        }
                    } else {
                        let next_loop = self.loop_count.saturating_add(1);
                        self.set_loop_count(next_loop);
                        self.current_step = 0;
                    }
                } else {
                    self.current_step = next_step;
                }
            }

            self.first_gate_received = true;
            self.samples_since_gate = 0;

            if !self.finished {
                self.start_step();
            }
        }

        // Grace machinery ticks between clock edges: fire a pre-scheduled
        // before-the-beat chain at the tail of this step, and hand the
        // stream back to a deferred principal once its on-beat chain ends.
        if !self.finished {
            if self.grace_countdown > 0 {
                self.grace_countdown -= 1;
                if self.grace_countdown == 0 {
                    if self.gate_continuous || self.gate_samples_remaining > 0 {
                        // The cap in maybe_preschedule_next should have
                        // released already; force the edge the chain's
                        // first onset needs.
                        self.retrigger_dip = true;
                        self.gate_continuous = false;
                        self.gate_samples_remaining = 0;
                    }
                    self.grace_prescheduled = true;
                    self.grace_player.start(
                        self.pending_grace,
                        self.pending_grace_velocity,
                        self.pending_grace_per,
                    );
                }
            }

            if self.deferred_note.is_some() && !self.grace_player.is_active() {
                self.active_note = self.deferred_note.take();
                self.active_velocity = self.deferred_velocity;
                if self.deferred_gate_continuous {
                    self.deferred_gate_continuous = false;
                    self.gate_continuous = true;
                    self.gate_samples_remaining = 0;
                } else {
                    let mut gate_samples = self.deferred_gate_samples;
                    if self.grace_countdown > 0 {
                        // A chain for the next step is already counting
                        // down; release before its first onset.
                        gate_samples = gate_samples.min(
                            self.grace_countdown
                                .saturating_sub(release_gap(self.pending_grace_per)),
                        );
                    }
                    self.gate_samples_remaining = gate_samples;
                }
            }
        }
        let grace_voice = if self.finished {
            None
        } else {
            self.grace_player.tick()
        };

        self.samples_since_gate += 1;

        if self.gate_samples_remaining > 0 {
            self.gate_samples_remaining -= 1;
        }

        self.update_outputs(i, grace_voice);

        self.last_gate_in = self.inputs.clock(i);
        self.last_reset_in = self.inputs.reset_gate(i);
        self.last_next_cell_in = self.inputs.next_cell(i);
        self.last_previous_cell_in = self.inputs.previous_cell(i);
    }
}

impl Module for CellSequencer {
    fn name(&self) -> &str {
        "CellSequencer"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Mode is control-plane state: read it once per block so the
        // per-sample loop carries no atomic load for it. Likewise the grace
        // controls, converting seconds to samples once per block.
        let one_shot = self.one_shot;
        self.grace_samples_cfg = (self.grace_duration * self.sample_rate as f32) as u32;
        self.grace_velocity_cfg = self.grace_velocity;
        self.grace_on_beat_cfg = self.grace_on_beat;
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

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        self.apply_control(control, value)
    }

    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        self.apply_bank(control, payload)
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
mod request_tests;
#[cfg(test)]
mod tests;
