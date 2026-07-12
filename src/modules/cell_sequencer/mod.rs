//! Cell sequencer module for pattern-bank playback.
//!
//! The cell sequencer extends the deterministic step sequencer with multiple
//! stored sequences and controls for selecting or advancing between them.
//!
//! # Dynamics
//!
//! The `velocity` output carries the most recently struck step's `amplitude`
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
//! - `grace_duration_ms` (default 60): duration of each grace. The whole
//!   chain is clamped to half the measured step duration, shrinking
//!   proportionally rather than dropping graces.
//! - `grace_velocity` (default 0.8): velocity scale relative to the
//!   decorated step's `amplitude`.
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
//! and latches the `end` output gate high (exactly one rising edge). Explicit
//! cell commands (`select_sequence`, `next`/`previous`, `advance`) still work
//! in one_shot and take priority over the auto-advance; `reset_gate` or
//! selecting a cell re-arms a finished sequencer.

use serde_json::Value;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::traits::ControlMeta;
use crate::Module;

use super::step_sequencer::grace::{clamp_per_grace, release_gap, GracePlayer, GraceVoice};
pub(crate) use super::step_sequencer::grace::{
    DEFAULT_GRACE_DURATION_MS, DEFAULT_GRACE_VELOCITY, MAX_GRACE_DURATION_MS, MIN_GRACE_DURATION_MS,
};
use super::step_sequencer::{parse_pattern, GraceChain, Step};

pub use self::controls::CellSequencerControls;

mod controls;
mod inputs;
mod outputs;
mod parse;

pub(crate) use parse::parse_sequence_bank_json;
use parse::{normalize_sequence_index, parse_sequence_bank};

pub const DEFAULT_STEPS: usize = 16;
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;
pub const DEFAULT_BASE_NOTE: u8 = 48;
pub const MAX_SEQUENCES: usize = 64;
/// Upper bound on steps per cell. This exists to bound the bank clone the
/// audio thread performs when the sequence bank changes (and to sanity-bound
/// `sequences_json` input), not playback cost — playback is O(1) per sample
/// regardless of length. 1024 keeps the worst-case swap around ~1 MB while
/// letting a full through-composed lane live in a single cell.
pub const MAX_STEPS: usize = 1024;

pub struct CellSequencer {
    sample_rate: u32,
    ctrl: CellSequencerControls,
    sequences: Vec<Vec<Step>>,
    last_sequence_bank_version: u64,
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
    /// One-shot playback of the bank has completed; `end` latches high and
    /// clock edges are ignored until reset or an explicit cell selection.
    finished: bool,
    /// Force the output gate low for exactly this sample: a new note is
    /// starting while the previous step's gate is still sounding (its
    /// duration was over-estimated — the cold start before the second clock
    /// edge, or a sudden accelerando), so the downstream envelope needs an
    /// explicit release edge to retrigger. Cleared by `update_outputs`.
    retrigger_dip: bool,
    /// Velocity of the most recently struck note (the step's `amplitude`,
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
    last_next_sequence_in: f32,
    last_previous_sequence_in: f32,
    last_control_selected_sequence: usize,
    last_advance_request_count: u64,
    cached_frequency: f32,
    cached_frequency_offset: Option<i8>,
    cached_frequency_base: u8,
    inputs: inputs::CellSequencerInputs,
    outputs: outputs::CellSequencerOutputs,
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
            last_next_sequence_in: 0.0,
            last_previous_sequence_in: 0.0,
            last_control_selected_sequence: current_sequence,
            last_advance_request_count: controls.advance_request_count(),
            cached_frequency: 0.0,
            cached_frequency_offset: None,
            cached_frequency_base: controls.base_note(),
            inputs: inputs::CellSequencerInputs::new(),
            outputs: outputs::CellSequencerOutputs::new(),
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

    /// Enables or disables one-shot playback (the `mode` control).
    pub fn with_one_shot(self, one_shot: bool) -> Self {
        self.ctrl.set_one_shot(one_shot);
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
            self.gate_continuous = false;
            self.active_note = None;
            self.clear_grace_state();
            self.last_control_selected_sequence = 0;
            self.ctrl.set_current_cell(0);
            self.ctrl.set_loop_count(0);
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
        self.ctrl.set_current_cell(self.current_sequence);
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
        let midi_note = (self.ctrl.base_note() as i16 + offset as i16).clamp(0, 127) as u8;
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
                self.ctrl.gate_length()
            } else {
                1.0
            }
        } else {
            step.gate_length.unwrap_or(self.ctrl.gate_length())
        };
        (self.step_duration_samples as f32 * gate_length) as u32
    }

    fn start_step(&mut self) {
        // Whether the previous step's gate is still sounding at this edge. A
        // correctly measured step releases before the boundary (the FUG-189
        // gap), but an over-estimated duration — the cold start before the
        // second clock edge, or a sudden accelerando — can leave it high.
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
                    let velocity = step.amplitude.unwrap_or(1.0);
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
                    self.active_velocity = step.amplitude.unwrap_or(1.0);
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
        self.pending_grace_velocity = next.amplitude.unwrap_or(1.0) * self.grace_velocity_cfg;
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

    /// Ends one-shot playback: silence the voice and latch the `end` gate.
    fn finish(&mut self) {
        self.set_finished(true);
        self.active_note = None;
        self.gate_samples_remaining = 0;
        self.gate_continuous = false;
        self.clear_grace_state();
    }

    /// Updates `finished` and mirrors it into the controls' read-only `ended`
    /// flag (an event-rate store, not per sample).
    fn set_finished(&mut self, finished: bool) {
        self.finished = finished;
        self.ctrl.set_ended(finished);
    }

    fn effective_selected_sequence(&self, i: usize) -> usize {
        self.inputs
            .select_sequence(i, self.ctrl.selected_sequence())
    }

    fn effective_wait_for_cycle_end(&self, i: usize) -> bool {
        self.inputs
            .wait_for_cycle_end(i, self.ctrl.wait_for_cycle_end())
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
        self.ctrl.set_selected_sequence(sequence_index);
        self.last_control_selected_sequence = sequence_index;
        self.ctrl.set_current_cell(sequence_index);
        self.ctrl.set_loop_count(0);
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
            self.ctrl.set_selected_sequence(0);
            self.last_control_selected_sequence = 0;
            return;
        }

        let sequence_index = normalize_sequence_index(sequence_index, self.sequences.len());
        self.ctrl.set_selected_sequence(sequence_index);
        self.last_control_selected_sequence = sequence_index;

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
        let base = self.ctrl.selected_sequence() as isize;
        (base + offset).rem_euclid(len) as usize
    }

    fn step_count(&self) -> usize {
        self.ctrl.steps()
    }

    fn frequency_for_active_note(&mut self) -> f32 {
        let Some(offset) = self.active_note else {
            self.cached_frequency_offset = None;
            return 0.0;
        };
        let base = self.ctrl.base_note();
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
        self.sync_sequences_from_controls();

        let selected_sequence =
            normalize_sequence_index(self.effective_selected_sequence(i), self.sequences.len());
        if selected_sequence != self.last_control_selected_sequence {
            self.request_sequence_change(i, selected_sequence);
        }

        let advance_count = self.ctrl.advance_request_count();
        let advance_rising = advance_count != self.last_advance_request_count;
        self.last_advance_request_count = advance_count;

        let gate_rising = self.inputs.gate(i) > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset_gate(i) > 0.5 && self.last_reset_in <= 0.5;
        let next_rising = (self.inputs.next_sequence(i) > 0.5 && self.last_next_sequence_in <= 0.5)
            || advance_rising;
        let previous_rising =
            self.inputs.previous_sequence(i) > 0.5 && self.last_previous_sequence_in <= 0.5;

        if reset_rising {
            self.current_step = 0;
            self.gate_samples_remaining = 0;
            self.gate_continuous = false;
            self.active_note = None;
            self.clear_grace_state();
            self.set_finished(false);
        }

        if next_rising {
            let target = self.advance_sequence_offset(1);
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
                        self.ctrl.set_selected_sequence(self.current_sequence);
                        self.last_control_selected_sequence = self.current_sequence;
                        self.active_note = None;
                        self.ctrl.set_current_cell(self.current_sequence);
                        self.ctrl.set_loop_count(0);
                        self.current_step = 0;
                    } else if one_shot {
                        if self.current_sequence + 1 < self.sequences.len() {
                            // Play the bank through: fall into the next cell.
                            self.current_sequence += 1;
                            self.ctrl.set_selected_sequence(self.current_sequence);
                            self.last_control_selected_sequence = self.current_sequence;
                            self.active_note = None;
                            self.ctrl.set_current_cell(self.current_sequence);
                            self.ctrl.set_loop_count(0);
                            self.current_step = 0;
                        } else {
                            // The final cell's last step has sounded for its
                            // full duration: the bank is over.
                            self.finish();
                        }
                    } else {
                        let next_loop = self.ctrl.loop_count().saturating_add(1);
                        self.ctrl.set_loop_count(next_loop);
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

        self.last_gate_in = self.inputs.gate(i);
        self.last_reset_in = self.inputs.reset_gate(i);
        self.last_next_sequence_in = self.inputs.next_sequence(i);
        self.last_previous_sequence_in = self.inputs.previous_sequence(i);
    }
}

impl Module for CellSequencer {
    fn name(&self) -> &str {
        "CellSequencer"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Mode is control-plane state: read it once per block so the
        // per-sample loop carries no atomic load for it. Likewise the grace
        // controls, converting ms to samples once per block.
        let one_shot = self.ctrl.one_shot();
        self.grace_samples_cfg =
            (self.ctrl.grace_duration_ms() * self.sample_rate as f32 / 1000.0) as u32;
        self.grace_velocity_cfg = self.ctrl.grace_velocity();
        self.grace_on_beat_cfg = self.ctrl.grace_on_beat();
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

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(DEFAULT_BASE_NOTE as f32),
            ControlMeta::new("steps", "Number of steps per sequence")
                .with_range(1.0, MAX_STEPS as f32)
                .with_default(DEFAULT_STEPS as f32),
            ControlMeta::new("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(DEFAULT_GATE_LENGTH),
            ControlMeta::new("selected_sequence", "Active sequence index")
                .with_range(0.0, MAX_SEQUENCES as f32 - 1.0)
                .with_default(self.ctrl.selected_sequence() as f32),
            ControlMeta::string(
                "mode",
                "Playback mode: loop repeats the active cell; one_shot plays the bank through once and fires the end gate",
            )
            .with_options(vec!["loop".to_string(), "one_shot".to_string()])
            .with_default("loop"),
            ControlMeta::new("grace_duration_ms", "Duration of a single grace note in ms")
                .with_range(MIN_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS)
                .with_default(DEFAULT_GRACE_DURATION_MS),
            ControlMeta::new(
                "grace_velocity",
                "Velocity scale for grace notes relative to the decorated step",
            )
            .with_range(0.0, 1.0)
            .with_default(DEFAULT_GRACE_VELOCITY),
            ControlMeta::new(
                "grace_placement",
                "Grace placement: 0 = before the beat, 1 = on the beat",
            )
            .with_range(0.0, 1.0)
            .with_default(0.0),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "base_note" => Ok(self.ctrl.base_note() as f32),
            "steps" => Ok(self.ctrl.steps() as f32),
            "gate_length" => Ok(self.ctrl.gate_length()),
            "selected_sequence" => Ok(self.ctrl.selected_sequence() as f32),
            // Numeric view of the string `mode` control: 0.0 = loop, 1.0 = one_shot.
            "mode" => Ok(if self.ctrl.one_shot() { 1.0 } else { 0.0 }),
            "grace_duration_ms" => Ok(self.ctrl.grace_duration_ms()),
            "grace_velocity" => Ok(self.ctrl.grace_velocity()),
            // Numeric view of `grace_placement`: 0.0 = before, 1.0 = on_beat.
            "grace_placement" => Ok(if self.ctrl.grace_on_beat() { 1.0 } else { 0.0 }),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "base_note" => self.ctrl.set_base_note(value as u8),
            "steps" => self.ctrl.set_steps(value as usize),
            "gate_length" => self.ctrl.set_gate_length(value),
            "selected_sequence" => self.ctrl.set_selected_sequence(value.max(0.0) as usize),
            "mode" => self.ctrl.set_one_shot(value > 0.5),
            "grace_duration_ms" => self.ctrl.set_grace_duration_ms(value),
            "grace_velocity" => self.ctrl.set_grace_velocity(value),
            "grace_placement" => self.ctrl.set_grace_on_beat(value > 0.5),
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
        if let Some(mode) = config.get("mode").and_then(|value| value.as_str()) {
            controls.set_mode(mode)?;
        }
        if let Some(ms) = config
            .get("grace_duration_ms")
            .and_then(|value| value.as_f64())
        {
            controls.set_grace_duration_ms(ms as f32);
        }
        if let Some(scale) = config
            .get("grace_velocity")
            .and_then(|value| value.as_f64())
        {
            controls.set_grace_velocity(scale as f32);
        }
        if let Some(placement) = config
            .get("grace_placement")
            .and_then(|value| value.as_str())
        {
            controls.set_grace_placement(placement)?;
        }
        let module = CellSequencer::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests;
