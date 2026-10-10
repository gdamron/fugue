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
//!         "root_note": 48,
//!         "step_count": 8,
//!         "gate_length": 0.5,
//!         "pattern": [
//!           { "note": 0 },
//!           { "note": null },
//!           { "note": 7, "gate_length": 0.8 },
//!           { "note": 5 },
//!           { "note": null },
//!           { "note": 0, "gate_length": 1.0 },
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
//!     { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" },
//!     { "from": "seq", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
//!     { "from": "seq", "from_port": "gate", "to": "vca", "to_port": "level" },
//!     { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
//!     { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
//!   ]
//! }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::payload::{Payload, Retired, Shared};
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{
    ENDED, GATE_LENGTH, GRACE_DURATION, GRACE_PLACEMENT, MAX_STEPS, MODE, PATTERN, ROOT_NOTE,
    STEP_COUNT, TABLE,
};

mod controls;
mod factory;
pub(crate) mod grace;
mod inputs;
mod outputs;
mod parse;
mod step;

pub use factory::StepSequencerFactory;
use grace::{
    clamp_per_grace, release_gap, GracePlayer, DEFAULT_GRACE_DURATION, MAX_GRACE_DURATION,
    MIN_GRACE_DURATION,
};
pub(crate) use parse::{parse_pattern, parse_step, StepError};
pub use step::{GraceChain, Step, MAX_GRACE_NOTES, STEP_KEYS};

/// Default number of steps in a pattern.
pub const DEFAULT_STEPS: usize = 16;

/// Default gate length as a ratio of step duration (0.0-1.0).
pub const DEFAULT_GATE_LENGTH: f32 = 0.5;

/// Default base MIDI note (C3).
pub const DEFAULT_ROOT_NOTE: u8 = 48;

/// A deterministic step sequencer for pattern playback.
///
/// Plays back a fixed pattern of notes, advancing one step per clock gate.
/// Supports per-step gate lengths and rests.
///
/// # Inputs
///
/// - `clock` - Clock input (rising edge advances step)
/// - `reset` - Reset input (rising edge resets to step 0 and re-arms one-shot)
///
/// # Outputs
///
/// - `frequency` - Current note frequency in Hz (0.0 during rest)
/// - `gate` - Output gate signal (high during note, low during rest)
/// - `step` - Current step number (0 to steps-1)
/// - `ended` - End-of-sequence gate: 0.0 while playing; latches to 1.0 when a
///   one-shot pattern completes (exactly one rising edge per playthrough) and
///   stays high until `reset` or a switch back to loop mode
///
/// # Grace notes
///
/// A step's `grace` chain plays as short separate attacks on the mono
/// frequency/gate stream ahead of the principal, each followed by a real
/// release gap so downstream envelopes retrigger. `grace_duration`
/// (seconds, default 0.06) sets each grace's length (the chain clamps to half the
/// measured step duration); `grace_placement` selects `before` (default:
/// steal the previous step's tail, principal on the grid) or `on_beat`
/// (chain starts at the edge and delays the principal). A decorated step
/// with no previous step to steal from falls back to on-beat realization;
/// a chain overrun by the next clock edge is truncated — the principal
/// always wins.
///
/// # Playback modes
///
/// The `mode` control selects `loop` (default; the pattern repeats forever)
/// or `one_shot`: the pattern plays once, the final step sounds for its full
/// duration, and on the next clock edge the sequencer falls silent and fires
/// `ended`. Further clock edges are ignored until `reset` re-arms it.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::modules::step_sequencer::{StepSequencer, Step};
///
/// let mut seq = StepSequencer::new(44100)
///     .with_root_note(48)
///     .with_step_count(8)
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
    // Controls, applied on the thread running the sequencer.
    root_note: u8,
    step_count: usize,
    gate_length: f32,
    /// One-shot playback (the `mode` control: loop | one_shot).
    one_shot: bool,
    /// Duration of a single grace note in seconds.
    grace_duration: f32,
    /// Grace placement (the `grace_placement` control): `false` = before
    /// the beat, `true` = on the beat.
    grace_on_beat: bool,
    cells: Arc<ControlCells>,
    /// The pattern, kept whole: a written one arrives as a payload, and
    /// the one it replaces is retired off the audio thread.
    pattern: Shared<Vec<Step>>,
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
    /// One-shot playback has completed; the `ended` output latches high and
    /// clock edges are ignored until reset (or a switch back to loop mode).
    finished: bool,
    /// Force the output gate low for exactly this sample: a new note is
    /// starting while the previous step's gate is still sounding (its
    /// duration was over-estimated — the cold start before the second clock
    /// edge, or a sudden accelerando), so the downstream envelope needs an
    /// explicit release edge to retrigger. Cleared when the outputs are set.
    retrigger_dip: bool,
    /// Plays a step's grace chain as short attacks ahead of its principal.
    grace_player: GracePlayer,
    /// Countdown to a pre-scheduled (before-the-beat) grace chain for the
    /// upcoming step, in samples from the current step's edge; 0 = none.
    grace_countdown: u32,
    pending_grace: GraceChain,
    pending_grace_per: u32,
    /// The upcoming step's chain actually started sounding off the previous
    /// step's tail (before placement), so `start_step` must not replay it on
    /// the beat. Left false when a pre-scheduled countdown never fired (the
    /// edge came early), which falls back to on-beat realization.
    grace_prescheduled: bool,
    /// Principal note waiting for an on-beat grace chain to finish, with
    /// its remaining gate samples.
    deferred_note: Option<i8>,
    deferred_gate_samples: u32,
    /// Block-rate caches of the grace controls (read once per `process`).
    grace_samples_cfg: u32,
    grace_on_beat_cfg: bool,

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
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_parts(sample_rate, cells, Shared::new(Vec::new()))
    }

    /// A step sequencer holding what `cells` hold, applied (so clamped),
    /// playing `pattern`.
    fn with_parts(sample_rate: u32, cells: Arc<ControlCells>, pattern: Shared<Vec<Step>>) -> Self {
        let mut seq = Self {
            sample_rate,
            root_note: DEFAULT_ROOT_NOTE,
            step_count: DEFAULT_STEPS,
            gate_length: DEFAULT_GATE_LENGTH,
            one_shot: false,
            grace_duration: DEFAULT_GRACE_DURATION,
            grace_on_beat: false,
            cells,
            pattern,
            current_step: 0,
            gate_samples_remaining: 0,
            step_duration_samples: sample_rate / 2, // Default ~120 BPM
            samples_since_gate: 0,
            first_gate_received: false,
            active_note: None,
            finished: false,
            retrigger_dip: false,
            grace_player: GracePlayer::idle(),
            grace_countdown: 0,
            pending_grace: GraceChain::default(),
            pending_grace_per: 0,
            grace_prescheduled: false,
            deferred_note: None,
            deferred_gate_samples: 0,
            grace_samples_cfg: 0,
            grace_on_beat_cfg: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::StepSequencerInputs::new(),
            outputs: outputs::StepSequencerOutputs::new(),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = seq.cells.load(index) {
                let _ = apply_declared(&mut seq, index, value);
            }
        }
        seq
    }

    fn set(&mut self, control: ControlIndex, value: RtValue) {
        let _ = apply_declared(self, control, value);
    }

    /// Sets the base MIDI note.
    pub fn with_root_note(mut self, root_note: u8) -> Self {
        self.set_root_note(root_note);
        self
    }

    /// Sets the number of steps in the pattern.
    pub fn with_step_count(mut self, step_count: usize) -> Self {
        self.set_step_count(step_count);
        self
    }

    /// Sets the default gate length (ratio of step duration, 0.0-1.0).
    pub fn with_gate_length(mut self, gate_length: f32) -> Self {
        self.set_gate_length(gate_length);
        self
    }

    /// Sets the pattern.
    pub fn with_pattern(mut self, pattern: Vec<Step>) -> Self {
        self.set_pattern(pattern);
        self
    }

    /// Enables or disables one-shot playback (the `mode` control).
    pub fn with_one_shot(mut self, one_shot: bool) -> Self {
        self.set(MODE, RtValue::U32(u32::from(one_shot)));
        self
    }

    /// Sets the base MIDI note.
    pub fn set_root_note(&mut self, root_note: u8) {
        self.set(ROOT_NOTE, RtValue::I32(i32::from(root_note)));
    }

    /// Sets the number of steps.
    pub fn set_step_count(&mut self, step_count: usize) {
        let step_count = step_count.min(MAX_STEPS as usize) as i32;
        self.set(STEP_COUNT, RtValue::I32(step_count));
    }

    /// Sets the default gate length.
    pub fn set_gate_length(&mut self, gate_length: f32) {
        self.set(GATE_LENGTH, RtValue::F32(gate_length));
    }

    /// Sets the pattern. Allocates, and frees the one it replaces: never
    /// call it on the audio thread.
    pub fn set_pattern(&mut self, pattern: Vec<Step>) {
        self.pattern = Shared::new(pattern);
    }

    /// Returns the current step index.
    pub fn current_step(&self) -> usize {
        self.current_step
    }

    /// Returns the number of steps.
    pub fn step_count(&self) -> usize {
        self.step_count
    }

    /// Gets the step at the given index, returning a rest if out of bounds.
    fn get_step(&self, index: usize) -> Step {
        self.pattern.get(index).cloned().unwrap_or_default()
    }

    /// Calculates the frequency for a note offset.
    fn note_frequency(&self, offset: i8) -> f32 {
        let base = self.root_note;
        let midi_note = (base as i16 + offset as i16).clamp(0, 127) as u8;
        Note::new(midi_note).frequency()
    }

    fn next_step_is_held(&self) -> bool {
        let next_step = self.current_step + 1;
        next_step < self.step_count && self.get_step(next_step).held
    }

    /// Calculates the gate duration in samples for the current step.
    fn calculate_gate_samples(&self, step: &Step) -> u32 {
        let gate_length = if step.held || self.next_step_is_held() {
            1.0
        } else {
            step.gate_length.unwrap_or(self.gate_length)
        };

        (self.step_duration_samples as f32 * gate_length) as u32
    }

    fn start_step(&mut self) {
        let step = self.get_step(self.current_step);
        let gate_still_high = self.gate_samples_remaining > 0 || self.grace_player.is_active();

        // The edge closes out any grace playback in flight: an overrunning
        // chain is truncated (the principal always wins), and a stale
        // pre-schedule whose countdown never fired is dropped.
        let prescheduled = self.grace_prescheduled;
        self.grace_prescheduled = false;
        self.grace_countdown = 0;
        self.grace_player.cancel();
        // An unsounded deferred principal is musically past; the new step wins.
        self.deferred_note = None;

        match step.note {
            Some(offset) => {
                // A new note while the previous gate is still sounding (its
                // duration was over-estimated) needs a forced release edge
                // to retrigger the downstream envelope.
                if gate_still_high {
                    self.retrigger_dip = true;
                }
                let chain_on_beat =
                    !step.grace.is_empty() && (self.grace_on_beat_cfg || !prescheduled);
                if chain_on_beat {
                    // Play the chain from this edge and defer the principal
                    // until it ends: on_beat placement, or the fallback when
                    // no previous step could host a before-the-beat chain
                    // (cold start or pattern wrap).
                    let per = clamp_per_grace(
                        &step.grace,
                        self.grace_samples_cfg,
                        self.step_duration_samples / 2,
                    );
                    self.grace_player.start(step.grace, 1.0, per);
                    self.deferred_note = Some(offset);
                    let chain_total = GracePlayer::chain_samples(&step.grace, per);
                    self.deferred_gate_samples = self
                        .calculate_gate_samples(&step)
                        .saturating_sub(chain_total);
                    self.gate_samples_remaining = 0;
                } else {
                    self.active_note = Some(offset);
                    self.gate_samples_remaining = self.calculate_gate_samples(&step);
                }
            }
            None if step.held && self.active_note.is_some() => {
                self.gate_samples_remaining = self.calculate_gate_samples(&step);
            }
            None => {
                self.active_note = None;
                self.gate_samples_remaining = 0;
            }
        }

        // Before-the-beat placement: a decorated upcoming step steals this
        // step's tail so its principal lands on the grid. Runs for every
        // step (a rest can host a chain too).
        self.maybe_preschedule_next();
    }

    /// Schedules the next step's grace chain (before-the-beat placement) to
    /// sound at the tail of the current step, ending at the predicted next
    /// edge, and releases this step's gate early enough that the chain's
    /// first onset presents a real rising edge. The look-ahead does not wrap
    /// the pattern: a decorated step 0 falls back to on-beat realization in
    /// `start_step` instead.
    fn maybe_preschedule_next(&mut self) {
        if self.grace_on_beat_cfg {
            return;
        }
        let next_index = self.current_step + 1;
        if next_index >= self.step_count {
            return;
        }
        let next = self.get_step(next_index);
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
        self.pending_grace_per = per;
        self.grace_countdown = start_at;

        // Release the current step's gate a gap before the chain's first
        // onset so the onset presents a real rising edge.
        let cap = start_at.saturating_sub(release_gap(per));
        self.gate_samples_remaining = self.gate_samples_remaining.min(cap);
        self.deferred_gate_samples = self.deferred_gate_samples.min(cap);
    }

    /// Silences any grace playback and scheduling (reset or end of one-shot).
    fn clear_grace_state(&mut self) {
        self.grace_player.cancel();
        self.grace_countdown = 0;
        self.grace_prescheduled = false;
        self.deferred_note = None;
    }

    /// Advances to the next step.
    fn advance_step(&mut self) {
        self.current_step = (self.current_step + 1) % self.step_count;
    }

    /// Resets to step 0 and re-arms one-shot playback.
    fn reset(&mut self) {
        self.current_step = 0;
        self.gate_samples_remaining = 0;
        self.active_note = None;
        self.clear_grace_state();
        self.set_finished(false);
    }

    /// Ends one-shot playback: silence the voice and latch the `ended` gate.
    fn finish(&mut self) {
        self.set_finished(true);
        self.active_note = None;
        self.gate_samples_remaining = 0;
        self.clear_grace_state();
    }

    /// Updates `finished` and publishes it as the read-only `ended` control
    /// (an event-rate store, not per sample).
    fn set_finished(&mut self, finished: bool) {
        self.finished = finished;
        self.cells.publish(ENDED, RtValue::Bool(finished));
    }

    /// Processes one sample.
    fn process_sample(&mut self, i: usize, one_shot: bool) {
        // Detect rising edges
        let gate_rising = self.inputs.clock(i) > 0.5 && self.last_gate_in <= 0.5;
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
                if one_shot && self.current_step + 1 >= self.step_count {
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

        // Grace machinery ticks between clock edges: fire a pre-scheduled
        // before-the-beat chain at the tail of this step, and hand the
        // stream back to a deferred principal once its on-beat chain ends.
        if !self.finished {
            if self.grace_countdown > 0 {
                self.grace_countdown -= 1;
                if self.grace_countdown == 0 {
                    if self.gate_samples_remaining > 0 {
                        // The cap in maybe_preschedule_next should have
                        // released already; force the edge the chain's
                        // first onset needs.
                        self.retrigger_dip = true;
                        self.gate_samples_remaining = 0;
                    }
                    self.grace_prescheduled = true;
                    self.grace_player
                        .start(self.pending_grace, 1.0, self.pending_grace_per);
                }
            }

            if self.deferred_note.is_some() && !self.grace_player.is_active() {
                self.active_note = self.deferred_note.take();
                let mut gate_samples = self.deferred_gate_samples;
                if self.grace_countdown > 0 {
                    // A chain for the next step is already counting down;
                    // release before its first onset.
                    gate_samples = gate_samples.min(
                        self.grace_countdown
                            .saturating_sub(release_gap(self.pending_grace_per)),
                    );
                }
                self.gate_samples_remaining = gate_samples;
            }
        }
        let grace_voice = if self.finished {
            None
        } else {
            self.grace_player.tick()
        };

        // Count samples for step duration measurement
        self.samples_since_gate += 1;

        // Update gate output (decrement remaining samples)
        if self.gate_samples_remaining > 0 {
            self.gate_samples_remaining -= 1;
        }

        // Update cached outputs. An active grace chain owns the mono
        // frequency/gate stream; the principal resumes when it ends.
        let (frequency, gate_high) = match grace_voice {
            Some(voice) => (self.note_frequency(voice.offset), voice.gate),
            None => (
                self.active_note
                    .map(|offset| self.note_frequency(offset))
                    .unwrap_or(0.0),
                self.gate_samples_remaining > 0,
            ),
        };
        self.outputs.set(
            i,
            frequency,
            if self.retrigger_dip {
                // One-sample forced release so the incoming note retriggers.
                0.0
            } else if gate_high {
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
        self.last_gate_in = self.inputs.clock(i);
        self.last_reset_in = self.inputs.reset(i);
    }
}

impl Module for StepSequencer {
    fn name(&self) -> &str {
        "StepSequencer"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Mode is control-plane state: read it once per block so the
        // per-sample loop carries no atomic load for it. Likewise the grace
        // controls, converting seconds to samples once per block.
        let one_shot = self.one_shot;
        self.grace_samples_cfg = (self.grace_duration * self.sample_rate as f32) as u32;
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

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        Ok(match (control, value) {
            (ROOT_NOTE, RtValue::I32(note)) => {
                self.root_note = note.clamp(0, 127) as u8;
                RtValue::I32(i32::from(self.root_note))
            }
            (STEP_COUNT, RtValue::I32(count)) => {
                self.step_count = count.clamp(1, MAX_STEPS) as usize;
                RtValue::I32(self.step_count as i32)
            }
            (GATE_LENGTH, RtValue::F32(length)) => {
                self.gate_length = length.clamp(0.0, 1.0);
                RtValue::F32(self.gate_length)
            }
            (MODE, RtValue::U32(mode @ 0..=1)) => {
                self.one_shot = mode == 1;
                value
            }
            (GRACE_DURATION, RtValue::F32(seconds)) => {
                self.grace_duration = seconds.clamp(MIN_GRACE_DURATION, MAX_GRACE_DURATION);
                RtValue::F32(self.grace_duration)
            }
            (GRACE_PLACEMENT, RtValue::U32(placement @ 0..=1)) => {
                self.grace_on_beat = placement == 1;
                value
            }
            (MODE | GRACE_PLACEMENT, RtValue::U32(_)) => return Err(Refusal::Invalid),
            _ => return Err(Refusal::Unsupported),
        })
    }

    /// Takes a written pattern from the next sample: the step playing keeps
    /// sounding, and the next clock edge reads the new pattern (a rest past
    /// its end, as ever).
    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        if control != PATTERN {
            return Err((Refusal::Unsupported, payload));
        }
        match payload.downcast::<Vec<Step>>() {
            Ok(pattern) => Ok(std::mem::replace(&mut self.pattern, pattern).into()),
            Err(payload) => Err((Refusal::Invalid, payload)),
        }
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
mod pattern_tests;
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
            .with_step_count(4)
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
            seq.set_input("clock", clock).unwrap();
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
